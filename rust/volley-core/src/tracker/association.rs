//! Maximum-weight bipartite matching with a private dummy column per detection.
//! The rectangular Hungarian algorithm is deterministic and O(n²m), where n
//! is detections and m is living tracks plus n possible unmatched detections.

/// For each detection, return a distinct matched active track index (if any).
/// A missing edge is never selected, and unmatched detections may start tracks.
pub(super) fn assign(scores: &[Vec<Option<f64>>]) -> Vec<Option<usize>> {
    let rows = scores.len();
    if rows == 0 {
        return Vec::new();
    }
    let tracks = scores[0].len();
    debug_assert!(scores.iter().all(|row| row.len() == tracks));
    if tracks == 0 {
        return vec![None; rows];
    }
    let columns = tracks + rows;
    let mut u = vec![0.0_f64; rows + 1];
    let mut v = vec![0.0_f64; columns + 1];
    let mut p = vec![0_usize; columns + 1];
    let mut way = vec![0_usize; columns + 1];
    for i in 1..=rows {
        p[0] = i;
        let mut column = 0;
        let mut min_cost = vec![f64::INFINITY; columns + 1];
        let mut used = vec![false; columns + 1];
        loop {
            used[column] = true;
            let current = p[column];
            let mut step = f64::INFINITY;
            let mut next = 0;
            for j in 1..=columns {
                if used[j] {
                    continue;
                }
                // Score 0 is equivalent to leaving the detection unassigned.
                // Prohibited edges cost more than an unmatched dummy.
                let cost = if j <= tracks {
                    scores[current - 1][j - 1].map_or(2.0, |score| 1.0 - score)
                } else {
                    1.0
                };
                let reduced = cost - u[current] - v[j];
                if reduced < min_cost[j] {
                    min_cost[j] = reduced;
                    way[j] = column;
                }
                if min_cost[j] < step {
                    step = min_cost[j];
                    next = j;
                }
            }
            for j in 0..=columns {
                if used[j] {
                    u[p[j]] += step;
                    v[j] -= step;
                } else {
                    min_cost[j] -= step;
                }
            }
            column = next;
            if p[column] == 0 {
                break;
            }
        }
        loop {
            let previous = way[column];
            p[column] = p[previous];
            column = previous;
            if column == 0 {
                break;
            }
        }
    }
    let mut result = vec![None; rows];
    for j in 1..=tracks {
        let i = p[j];
        if i != 0 && scores[i - 1][j - 1].is_some() {
            result[i - 1] = Some(j - 1);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::assign;

    #[test]
    fn global_matching_beats_greedy_and_does_not_duplicate_ids() {
        let result = assign(&[vec![Some(0.90), Some(0.80)], vec![Some(0.88), None]]);
        assert_eq!(result, vec![Some(1), Some(0)]);
    }

    #[test]
    fn unmatched_and_forbidden_pairs_never_consume_tracks() {
        assert_eq!(assign(&[vec![None], vec![Some(0.8)]]), vec![None, Some(0)]);
        assert_eq!(assign(&[vec![], vec![]]), vec![None, None]);
    }
}
