//! Cross-channel token counting.
//!
//! Single source of truth for the `token_count` field on every
//! `DocumentNode` / `SemanticTreeElement`. Centralizing this here keeps
//! per-node token counts comparable across input channels (PDF, MD,
//! DOCX): the same body text passed through any channel must produce
//! the same `token_count`.
//!
//! The current implementation is a rough character-based estimate
//! (`text.len() / 4`). It is not a real tokenizer; the placeholder
//! survives because cross-channel comparability matters more than
//! absolute accuracy for the current consumers. When a real tokenizer
//! (BPE, tiktoken, etc.) replaces it, both channels follow in lockstep
//! because they go through this one function.

/// Rough character-based token estimate. Approximation, not a real
/// tokenizer. Stable across channels; replaceable in one place.
///
/// Non-empty text is floored to 1 token: integer-divide `len/4` returns
/// 0 for any 1-3 char body (e.g. `---`, `*`, `>`), which under-reports
/// content that obviously carries at least one token. Empty text stays
/// at 0 (semantically correct: no content, no tokens). See DT-01 for
/// the standing argument that this estimator is the right shape until
/// a real BPE tokenizer is plumbed.
pub fn estimate_token_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        (text.len() / 4).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::estimate_token_count;

    #[test]
    fn estimate_is_len_over_four() {
        assert_eq!(estimate_token_count(""), 0);
        assert_eq!(estimate_token_count("abcd"), 1);
        assert_eq!(estimate_token_count("hello world"), 2); // 11 / 4 = 2
    }

    #[test]
    fn estimate_is_stable_for_same_input() {
        let s = "The quick brown fox jumps over the lazy dog.";
        assert_eq!(estimate_token_count(s), estimate_token_count(s));
    }

    #[test]
    fn estimate_floors_short_text_to_one() {
        // Non-empty text floors to 1; integer-divide would otherwise
        // under-report 1-3 char bodies as 0 tokens (e.g. `---`, `*`).
        assert_eq!(estimate_token_count("-"), 1);
        assert_eq!(estimate_token_count("---"), 1);
        // Empty text remains 0 (no content, no tokens).
        assert_eq!(estimate_token_count(""), 0);
    }
}
