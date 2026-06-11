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
    let matches = total_matches(a, b);
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

/// The total matched element count: every maximal matching block,
/// recursively split around the longest match, summed. The reference
/// also sorts and merges adjacent blocks for its block-list API, but
/// merging never changes the size sum, so the ratio surface ported
/// here omits that machinery rather than carrying it dead.
fn total_matches(a: &[char], b: &[char]) -> usize {
    let index = BIndex::new(b);
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    let mut total = 0usize;
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest_match(a, b, &index, alo, ahi, blo, bhi);
        if k > 0 {
            total += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    total
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
    fn multi_block_matches_recurse_on_both_sides() {
        assert_eq!(ratio("abcXXdefYY", "abcZZdefWW"), 0.6);
        assert_eq!(
            ratio("prefix mid suffix", "prefix XXX suffix"),
            0.8235294117647058
        );
        assert_eq!(ratio("abQcd", "abRcd"), 0.8);
    }

    #[test]
    fn popular_neighbours_extend_seeded_matches() {
        // Long b strings whose popular elements cannot seed matches
        // but still extend them through the adjacency passes; every
        // score is the reference library's output.
        let cases: [(String, String, f64); 4] = [
            (
                "xxhelloxx".into(),
                "x".repeat(120) + "hello" + &"x".repeat(120),
                0.07086614173228346,
            ),
            (
                "aahellozz".into(),
                "a".repeat(100) + "hello" + &"z".repeat(100),
                0.08411214953271028,
            ),
            (
                "hello".into(),
                "x".repeat(100) + "hello" + &"x".repeat(100),
                0.047619047619047616,
            ),
            (
                "xhellox world".into(),
                "x".repeat(150) + "hello world" + &"x".repeat(60),
                0.10256410256410256,
            ),
        ];
        for (a, b, expected) in cases {
            assert_eq!(ratio(&a, &b), expected, "{a:?}");
        }
    }

    #[test]
    fn seeds_at_the_window_edge_and_offset_blocks_count() {
        // A single-element seed at the very start of b.
        assert_eq!(ratio("z", "za"), 0.6666666666666666);
        // The longest match sits at an offset, so the recursion's
        // right window opens from mid-sequence on both sides.
        assert_eq!(ratio("xxabcdeXfg", "yyabcdeYfg"), 0.7);
        assert_eq!(ratio("Xfg hij", "Yfg hij"), 0.8571428571428571);
    }

    #[test]
    fn patterned_popular_padding_extends_direction_sensitively() {
        // Alternating popular padding around the seed: the adjacency
        // passes must walk the right direction and indexes, or the
        // pattern breaks the walk; both scores are the reference
        // library's output.
        let b1 = "ab".repeat(110) + "hello" + &"ab".repeat(10);
        assert_eq!(ratio("ababhelloabab", &b1), 0.10077519379844961);
        let b2 = "ba".repeat(105) + "hello" + &"ab".repeat(8);
        assert_eq!(ratio("abab hello baba", &b2), 0.04065040650406504);
    }

    #[test]
    fn blocked_origins_expose_the_adjacency_walk() {
        // The reference's extension passes also grow zero-seed matches
        // from a window's origin, so losses there can mask a broken
        // backward walk; these pairs put a mismatching prefix at the
        // origin so the walk's direction and indexes carry the score.
        assert_eq!(ratio("bz", "za"), 0.5);
        let b = "ab".repeat(110) + "hello" + &"ab".repeat(5);
        assert_eq!(ratio("qqabhelloab", &b), 0.07317073170731707);
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
