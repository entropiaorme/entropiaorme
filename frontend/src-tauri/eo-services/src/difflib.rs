//! `difflib.SequenceMatcher.ratio()`, ported for the quest-name fuzzy
//! match: the similarity score the mission-detection path thresholds.
//!
//! The port reproduces the reference algorithm over character
//! sequences with its default junk behaviour (no junk predicate;
//! autojunk active, so an element of `b` occurring more than
//! `len(b)/100 + 1` times is excluded from match seeding once `b`
//! reaches 200 elements). Scores are pinned against the reference
//! library's outputs in the tests.

use std::collections::HashMap;

/// The similarity ratio of two character sequences: twice the matched
/// element count over the total length (1.0 when both are empty).
pub fn sequence_ratio(a: &[char], b: &[char]) -> f64 {
    let matches: usize = matching_blocks(a, b).iter().map(|&(_, _, size)| size).sum();
    let length = a.len() + b.len();
    if length == 0 {
        return 1.0;
    }
    2.0 * matches as f64 / length as f64
}

/// The per-element index map over `b`, with the popular elements
/// (autojunk) split out once `b` is long enough.
struct BIndex {
    b2j: HashMap<char, Vec<usize>>,
}

impl BIndex {
    fn new(b: &[char]) -> Self {
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (index, &element) in b.iter().enumerate() {
            b2j.entry(element).or_default().push(index);
        }
        let n = b.len();
        if n >= 200 {
            let threshold = n / 100 + 1;
            b2j.retain(|_, indexes| indexes.len() <= threshold);
        }
        Self { b2j }
    }
}

/// The longest match within `a[alo..ahi]` and `b[blo..bhi]`, with the
/// reference's preference order (earliest in `a`, then earliest in
/// `b`) and its junk-adjacent extension passes (inert here: the junk
/// set is empty without a junk predicate; popular elements are only
/// excluded from seeding and still extend matches).
fn find_longest_match(
    a: &[char],
    b: &[char],
    index: &BIndex,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for (i, element) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut new_j2len: HashMap<usize, usize> = HashMap::new();
        if let Some(indexes) = index.b2j.get(element) {
            for &j in indexes {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = if j > 0 {
                    j2len.get(&(j - 1)).copied().unwrap_or(0) + 1
                } else {
                    1
                };
                new_j2len.insert(j, k);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        j2len = new_j2len;
    }
    // The non-junk extension passes (the junk passes are inert: no
    // junk predicate means an empty junk set).
    while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
        besti -= 1;
        bestj -= 1;
        bestsize += 1;
    }
    while besti + bestsize < ahi
        && bestj + bestsize < bhi
        && a[besti + bestsize] == b[bestj + bestsize]
    {
        bestsize += 1;
    }
    (besti, bestj, bestsize)
}

/// Every maximal matching block, recursively split around the longest
/// match, merged when adjacent, in the reference's order.
fn matching_blocks(a: &[char], b: &[char]) -> Vec<(usize, usize, usize)> {
    let index = BIndex::new(b);
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    let mut blocks: Vec<(usize, usize, usize)> = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest_match(a, b, &index, alo, ahi, blo, bhi);
        if k > 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort_unstable();

    let mut merged: Vec<(usize, usize, usize)> = Vec::new();
    let (mut i1, mut j1, mut k1) = (0usize, 0usize, 0usize);
    for (i2, j2, k2) in blocks {
        if i1 + k1 == i2 && j1 + k1 == j2 {
            k1 += k2;
        } else {
            if k1 > 0 {
                merged.push((i1, j1, k1));
            }
            (i1, j1, k1) = (i2, j2, k2);
        }
    }
    if k1 > 0 {
        merged.push((i1, j1, k1));
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ratio(a: &str, b: &str) -> f64 {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        sequence_ratio(&a, &b)
    }

    // Every expected score below is the reference library's output
    // over the same pair, computed externally.
    #[test]
    fn ratios_match_the_reference_library() {
        assert_eq!(ratio("", ""), 1.0);
        assert_eq!(ratio("abc", ""), 0.0);
        assert_eq!(ratio("", "abc"), 0.0);
        assert_eq!(ratio("abc", "abc"), 1.0);
        assert_eq!(ratio("abcd", "bcde"), 0.75);
        assert_eq!(ratio("abcabba", "cbabac"), 0.46153846153846156);
        assert_eq!(ratio("iron challenge", "the iron challenge"), 0.875);
        assert_eq!(
            ratio("daily hunt: atrox", "atrox daily"),
            0.35714285714285715
        );
        assert_eq!(
            ratio("a small obstacle", "a smal obstacle"),
            0.967741935483871
        );
        assert_eq!(ratio("qwxyz", "zyxwq"), 0.2);
        assert_eq!(ratio("aaaaab", "baaaaa"), 0.8333333333333334);
    }

    #[test]
    fn long_inputs_drop_popular_seeds_via_autojunk() {
        // b is 200 elements with 'a' appearing far beyond the
        // popularity threshold, so it cannot seed matches and only
        // the literal tail aligns.
        let a = "a".repeat(7) + "xyz";
        let b = "a".repeat(197) + "xyz";
        assert_eq!(ratio(&a, &b), 0.09523809523809523);
    }
}
