//! The weighted fuzzy scorer the skill-name resolution uses, ported
//! from the reference implementation of `rapidfuzz.fuzz.WRatio` (the
//! library's own pure-Python form, the binding the original calls):
//! the Indel ratio, the sliding-window partial ratio with its
//! boundary-character window skip, the token sort/set combinators
//! over whitespace-split sorted tokens, and the weighted combination
//! with the 0.95 unbase scale and the 1.5/8.0 length-ratio regime.
//!
//! Scores are pinned against values computed by the original library
//! (rapidfuzz 3.14) in the tests below; the underlying Indel
//! primitives come from the library author's own Rust port.

use rapidfuzz::distance::indel;

const UNBASE_SCALE: f64 = 0.95;

/// `fuzz.ratio`: the normalised Indel similarity over code points.
pub fn ratio(a: &[char], b: &[char]) -> f64 {
    indel::normalized_similarity(a.iter().copied(), b.iter().copied()) * 100.0
}

/// The implementation half of `partial_ratio`, assuming
/// `len(needle) <= len(haystack)`: the best window score, evaluating
/// only windows whose boundary character occurs in the needle (the
/// reference implementation's exact window set).
fn partial_ratio_impl(needle: &[char], haystack: &[char]) -> f64 {
    let len1 = needle.len();
    let len2 = haystack.len();
    let needle_set: std::collections::HashSet<char> = needle.iter().copied().collect();
    let mut best = 0.0f64;

    let mut consider = |window: &[char], best: &mut f64| {
        let score = indel::normalized_similarity(needle.iter().copied(), window.iter().copied());
        if score > *best {
            *best = score;
        }
    };

    for i in 1..len1 {
        if !needle_set.contains(&haystack[i - 1]) {
            continue;
        }
        consider(&haystack[..i], &mut best);
        if best == 1.0 {
            return 100.0;
        }
    }
    for i in 0..len2.saturating_sub(len1) {
        if !needle_set.contains(&haystack[i + len1 - 1]) {
            continue;
        }
        consider(&haystack[i..i + len1], &mut best);
        if best == 1.0 {
            return 100.0;
        }
    }
    for i in len2.saturating_sub(len1)..len2 {
        if !needle_set.contains(&haystack[i]) {
            continue;
        }
        consider(&haystack[i..], &mut best);
        if best == 1.0 {
            return 100.0;
        }
    }
    best * 100.0
}

/// `fuzz.partial_ratio`: the optimal alignment of the shorter string
/// inside the longer one (with the equal-length second pass the
/// reference runs both ways).
pub fn partial_ratio(a: &[char], b: &[char]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 100.0;
    }
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut score = partial_ratio_impl(shorter, longer);
    if score != 100.0 && a.len() == b.len() {
        let swapped = partial_ratio_impl(longer, shorter);
        if swapped > score {
            score = swapped;
        }
    }
    score
}

/// Python's `str.split()` over the original's whitespace class.
fn split_tokens(s: &[char]) -> Vec<Vec<char>> {
    let mut tokens = Vec::new();
    let mut current = Vec::new();
    for &ch in s {
        if ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch) {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn join_tokens(tokens: &[Vec<char>]) -> Vec<char> {
    let mut joined = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            joined.push(' ');
        }
        joined.extend_from_slice(token);
    }
    joined
}

fn sorted_unique(tokens: &[Vec<char>]) -> Vec<Vec<char>> {
    let mut unique: Vec<Vec<char>> = Vec::new();
    for token in tokens {
        if !unique.contains(token) {
            unique.push(token.clone());
        }
    }
    unique.sort();
    unique
}

/// `fuzz.token_sort_ratio`: the plain ratio over the
/// whitespace-sorted token join.
pub fn token_sort_ratio(a: &[char], b: &[char]) -> f64 {
    let mut tokens_a = split_tokens(a);
    let mut tokens_b = split_tokens(b);
    tokens_a.sort();
    tokens_b.sort();
    ratio(&join_tokens(&tokens_a), &join_tokens(&tokens_b))
}

fn norm_distance(dist: usize, lensum: usize) -> f64 {
    if lensum == 0 {
        100.0
    } else {
        100.0 - 100.0 * dist as f64 / lensum as f64
    }
}

/// `fuzz.token_set_ratio`: the unique/common word comparison.
pub fn token_set_ratio(a: &[char], b: &[char]) -> f64 {
    let tokens_a = sorted_unique(&split_tokens(a));
    let tokens_b = sorted_unique(&split_tokens(b));
    if tokens_a.is_empty() || tokens_b.is_empty() {
        return 0.0;
    }
    let intersect: Vec<Vec<char>> = tokens_a
        .iter()
        .filter(|token| tokens_b.contains(token))
        .cloned()
        .collect();
    let diff_ab: Vec<Vec<char>> = tokens_a
        .iter()
        .filter(|token| !tokens_b.contains(token))
        .cloned()
        .collect();
    let diff_ba: Vec<Vec<char>> = tokens_b
        .iter()
        .filter(|token| !tokens_a.contains(token))
        .cloned()
        .collect();

    // One sentence is part of the other one.
    if !intersect.is_empty() && (diff_ab.is_empty() || diff_ba.is_empty()) {
        return 100.0;
    }

    let diff_ab_joined = join_tokens(&diff_ab);
    let diff_ba_joined = join_tokens(&diff_ba);
    let ab_len = diff_ab_joined.len();
    let ba_len = diff_ba_joined.len();
    let sect_len = join_tokens(&intersect).len();

    let sect_ab_len = sect_len + usize::from(sect_len != 0) + ab_len;
    let sect_ba_len = sect_len + usize::from(sect_len != 0) + ba_len;

    let dist = indel::distance(
        diff_ab_joined.iter().copied(),
        diff_ba_joined.iter().copied(),
    );
    let result = norm_distance(dist, sect_ab_len + sect_ba_len);

    if sect_len == 0 {
        return result;
    }

    // Only the common section is shared, so those distances follow
    // from the length difference alone.
    let sect_ab_dist = usize::from(sect_len != 0) + ab_len;
    let sect_ab_ratio = norm_distance(sect_ab_dist, sect_len + sect_ab_len);
    let sect_ba_dist = usize::from(sect_len != 0) + ba_len;
    let sect_ba_ratio = norm_distance(sect_ba_dist, sect_len + sect_ba_len);

    result.max(sect_ab_ratio).max(sect_ba_ratio)
}

/// `fuzz.token_ratio`: the better of the sort and set forms.
pub fn token_ratio(a: &[char], b: &[char]) -> f64 {
    token_set_ratio(a, b).max(token_sort_ratio(a, b))
}

/// `fuzz.partial_token_ratio` as the reference inlines it: 100 on any
/// shared word, else the partial ratio over the sorted token joins,
/// widened by the sorted difference joins when they differ.
pub fn partial_token_ratio(a: &[char], b: &[char]) -> f64 {
    let tokens_split_a = split_tokens(a);
    let tokens_split_b = split_tokens(b);
    let unique_a = sorted_unique(&tokens_split_a);
    let unique_b = sorted_unique(&tokens_split_b);

    if unique_a.iter().any(|token| unique_b.contains(token)) {
        return 100.0;
    }

    let diff_ab: Vec<Vec<char>> = unique_a
        .iter()
        .filter(|token| !unique_b.contains(token))
        .cloned()
        .collect();
    let diff_ba: Vec<Vec<char>> = unique_b
        .iter()
        .filter(|token| !unique_a.contains(token))
        .cloned()
        .collect();

    let mut sorted_a = tokens_split_a.clone();
    sorted_a.sort();
    let mut sorted_b = tokens_split_b.clone();
    sorted_b.sort();
    let result = partial_ratio(&join_tokens(&sorted_a), &join_tokens(&sorted_b));

    if tokens_split_a.len() == diff_ab.len() && tokens_split_b.len() == diff_ba.len() {
        return result;
    }

    result.max(partial_ratio(
        &join_tokens(&diff_ab),
        &join_tokens(&diff_ba),
    ))
}

/// `fuzz.WRatio`: the weighted combination the original's resolver
/// scores candidates with.
pub fn wratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (len1, len2) = (a.len() as f64, b.len() as f64);
    let len_ratio = if len1 > len2 {
        len1 / len2
    } else {
        len2 / len1
    };

    let mut end_ratio = ratio(&a, &b);
    if len_ratio < 1.5 {
        return end_ratio.max(token_ratio(&a, &b) * UNBASE_SCALE);
    }

    let partial_scale = if len_ratio <= 8.0 { 0.9 } else { 0.6 };
    end_ratio = end_ratio.max(partial_ratio(&a, &b) * partial_scale);
    end_ratio.max(partial_token_ratio(&a, &b) * UNBASE_SCALE * partial_scale)
}

/// `process.extract(..., limit=top_n)`: every vocab entry scored,
/// ordered by score descending with ties kept in vocab order.
pub fn extract_top<'a>(query: &str, vocab: &'a [String], top_n: usize) -> Vec<(&'a str, f64)> {
    let mut scored: Vec<(usize, &'a str, f64)> = vocab
        .iter()
        .enumerate()
        .map(|(index, entry)| (index, entry.as_str(), wratio(query, entry)))
        .collect();
    scored.sort_by(|left, right| {
        right
            .2
            .partial_cmp(&left.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.0.cmp(&right.0))
    });
    scored
        .into_iter()
        .take(top_n)
        .map(|(_, entry, score)| (entry, score))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    /// Every expected value below was computed by the original
    /// library (rapidfuzz 3.14) over the same inputs.
    #[test]
    fn the_sub_scorers_match_the_original_library() {
        let cases: [(&str, &str, f64, f64, f64, f64); 11] = [
            ("Anatomy", "Anatomy", 100.0, 100.0, 100.0, 100.0),
            (
                "Anatom",
                "Anatomy",
                92.3076923076923,
                100.0,
                92.3076923076923,
                92.3076923076923,
            ),
            (
                "Rifle",
                "Riffle",
                90.9090909090909,
                80.0,
                90.9090909090909,
                90.9090909090909,
            ),
            (
                "Food Technology",
                "Wood Technology",
                93.33333333333333,
                96.55172413793103,
                66.66666666666667,
                93.33333333333333,
            ),
            (
                "Food Technology",
                "Technology of Food",
                60.60606060606061,
                80.0,
                90.9090909090909,
                100.0,
            ),
            (
                "Laser Weaponry Technology",
                "Weaponry Technology",
                86.36363636363636,
                100.0,
                86.36363636363636,
                100.0,
            ),
            (
                "BLP",
                "BLP Weaponry Technology",
                23.076923076923073,
                100.0,
                23.076923076923073,
                100.0,
            ),
            (
                "Sweat Gatherer",
                "Sweating",
                45.45454545454546,
                76.92307692307692,
                45.45454545454546,
                45.45454545454545,
            ),
            ("abcdefgh", "xyz", 0.0, 0.0, 0.0, 0.0),
            (
                "a",
                "abcdefghijklmnopqrstuvwxyz",
                7.4074074074074066,
                100.0,
                7.4074074074074066,
                7.407407407407405,
            ),
            (
                "Combat Reflexes",
                "Combat  Reflexes",
                96.7741935483871,
                93.33333333333333,
                100.0,
                100.0,
            ),
        ];
        for (a, b, want_ratio, want_partial, want_sort, want_set) in cases {
            let (a, b) = (chars(a), chars(b));
            assert_close(ratio(&a, &b), want_ratio);
            assert_close(partial_ratio(&a, &b), want_partial);
            assert_close(token_sort_ratio(&a, &b), want_sort);
            assert_close(token_set_ratio(&a, &b), want_set);
        }
    }

    #[test]
    fn wratio_matches_the_original_library() {
        let cases: [(&str, &str, f64); 12] = [
            ("Anatomy", "Anatomy", 100.0),
            ("Anatom", "Anatomy", 92.3076923076923),
            ("Rifle", "Riffle", 90.9090909090909),
            ("Food Technology", "Wood Technology", 93.33333333333333),
            ("Food Technology", "Technology of Food", 95.0),
            ("Laser Weaponry Technology", "Weaponry Technology", 95.0),
            ("BLP", "BLP Weaponry Technology", 90.0),
            ("Sweat Gatherer", "Sweating", 69.23076923076923),
            ("abcdefgh", "xyz", 0.0),
            ("a", "abcdefghijklmnopqrstuvwxyz", 60.0),
            ("Combat Reflexes", "Combat  Reflexes", 96.7741935483871),
            ("evade", "Evade", 80.0),
        ];
        for (a, b, want) in cases {
            assert_close(wratio(a, b), want);
        }
    }

    #[test]
    fn extraction_orders_by_score_with_stable_ties() {
        let vocab: Vec<String> = [
            "Anatomy",
            "Rifle",
            "Wood Technology",
            "Combat Reflexes",
            "Evade",
            "BLP Weaponry Technology",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        // Both result sets were computed by the original library.
        let top = extract_top("Food Technology", &vocab, 3);
        assert_eq!(top[0].0, "Wood Technology");
        assert_close(top[0].1, 93.33333333333333);
        assert_eq!(top[1].0, "BLP Weaponry Technology");
        assert_close(top[1].1, 85.5);
        assert_eq!(top[2].0, "Anatomy");
        assert_close(top[2].1, 41.53846153846154);

        let top = extract_top("rifel", &vocab, 3);
        assert_eq!(top[0].0, "Rifle");
        assert_close(top[0].1, 60.0);
        // The 36.0 tie keeps vocab order.
        assert_eq!(top[1].0, "Combat Reflexes");
        assert_close(top[1].1, 36.0);
        assert_eq!(top[2].0, "BLP Weaponry Technology");
        assert_close(top[2].1, 36.0);

        assert!(extract_top("anything", &[], 3).is_empty());
    }
}
