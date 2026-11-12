//! Did-you-mean helpers matching the historical Roots CLI.

/// Compute Levenshtein edit distance over Unicode scalar values.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (index, left) in a.iter().enumerate() {
        let mut current = Vec::with_capacity(b.len() + 1);
        current.push(index + 1);
        for (column, right) in b.iter().enumerate() {
            let cost = usize::from(left != right);
            let value = (current[column] + 1)
                .min(previous[column + 1] + 1)
                .min(previous[column] + cost);
            current.push(value);
        }
        previous = current;
    }
    previous[b.len()]
}

/// Return the closest candidate within `max_distance`; a unique prefix wins.
pub fn closest<'a>(input: &str, candidates: &'a [&str], max_distance: usize) -> Option<&'a str> {
    if input.chars().count() >= 2 {
        let prefixed: Vec<&str> = candidates
            .iter()
            .copied()
            .filter(|candidate| candidate.starts_with(input))
            .collect();
        if prefixed.len() == 1 {
            return prefixed.first().copied();
        }
    }

    let mut best: Option<&str> = None;
    let mut best_distance = usize::MAX;
    for candidate in candidates {
        let distance = levenshtein(input, candidate);
        if distance < best_distance {
            best_distance = distance;
            best = Some(candidate);
        }
    }
    if best_distance <= max_distance {
        best
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_a_unique_prefix() {
        // A non-unique prefix falls back to edit distance, just as the
        // TypeScript implementation does; `start` is then the closest.
        assert_eq!(closest("sta", &["status", "start"], 2), Some("start"));
        assert_eq!(closest("stat", &["status", "start"], 2), Some("status"));
    }

    #[test]
    fn returns_a_close_spelling() {
        assert_eq!(closest("stauts", &["status"], 2), Some("status"));
    }
}
