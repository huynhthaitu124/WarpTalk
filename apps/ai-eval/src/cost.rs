//! Indicative token prices (USD per 1M tokens) for the cost estimate. The table is a rough
//! guide and goes stale: pass `--price-in` / `--price-out` for exact numbers.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input_per_m: f64,
    pub output_per_m: f64,
}

const TABLE: &[(&str, f64, f64)] = &[
    ("gpt-4o-mini", 0.15, 0.60),
    ("gpt-4.1-mini", 0.40, 1.60),
    ("gpt-4.1-nano", 0.10, 0.40),
    ("gpt-4.1", 2.00, 8.00),
    ("gpt-4o", 2.50, 10.00),
    ("claude-haiku", 1.00, 5.00),
    ("claude-sonnet", 3.00, 15.00),
    ("gemini-2.5-flash-lite", 0.10, 0.40),
    ("gemini-2.5-flash", 0.30, 2.50),
    ("gemini-2.5-pro", 1.25, 10.00),
];

/// Longest-prefix match so `gpt-4.1-mini` does not fall into `gpt-4.1`.
pub fn lookup(model: &str) -> Option<Price> {
    let m = model.to_lowercase();
    TABLE
        .iter()
        .filter(|(p, _, _)| m.starts_with(p))
        .max_by_key(|(p, _, _)| p.len())
        .map(|&(_, i, o)| Price {
            input_per_m: i,
            output_per_m: o,
        })
}

pub fn estimate(price: Option<Price>, input_tokens: u64, output_tokens: u64) -> Option<f64> {
    price.map(|p| {
        (input_tokens as f64 * p.input_per_m + output_tokens as f64 * p.output_per_m) / 1e6
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_wins() {
        assert_eq!(lookup("gpt-4.1-mini-2025").unwrap().input_per_m, 0.40);
        assert_eq!(lookup("gpt-4.1").unwrap().input_per_m, 2.00);
        assert!(lookup("fake-1").is_none());
        let c = estimate(lookup("gpt-4o-mini"), 1_000_000, 1_000_000).unwrap();
        assert!((c - 0.75).abs() < 1e-9);
    }
}
