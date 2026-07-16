//! Price-outlier detection against the rolling median of recent observed
//! prices for the same (family, model). Too-good-to-be-true prices are a
//! scam signal — flagged, never dropped.

/// Below this many observations the median is noise — never flag.
pub const MIN_HISTORY: usize = 5;

/// Median price in cents. Even-length inputs return the lower middle —
/// exactness doesn't matter for a threshold signal.
pub fn median(prices: &[i64]) -> Option<i64> {
    if prices.is_empty() {
        return None;
    }
    let mut sorted = prices.to_vec();
    sorted.sort_unstable();
    Some(sorted[(sorted.len() - 1) / 2])
}

/// True when `price` is below `ratio × median(history)` and there is
/// enough history to trust the median.
pub fn is_outlier(price: i64, history: &[i64], ratio: f64) -> bool {
    if history.len() < MIN_HISTORY {
        return false;
    }
    match median(history) {
        Some(m) => (price as f64) < (m as f64) * ratio,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_and_even_counts() {
        assert_eq!(median(&[3, 1, 2]), Some(2));
        assert_eq!(median(&[4, 1, 2, 3]), Some(2)); // lower of the two middles
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn outlier_when_far_below_median() {
        // median 50_000, ratio 0.5 → outlier under 25_000
        let history = [48_000, 50_000, 52_000, 51_000, 49_000];
        assert!(is_outlier(20_000, &history, 0.5));
        assert!(!is_outlier(30_000, &history, 0.5));
        assert!(!is_outlier(50_000, &history, 0.5));
    }

    #[test]
    fn no_outlier_with_thin_history() {
        // fewer than MIN_HISTORY observations → never an outlier
        assert!(!is_outlier(1, &[50_000, 51_000], 0.5));
        assert!(!is_outlier(1, &[], 0.5));
    }
}
