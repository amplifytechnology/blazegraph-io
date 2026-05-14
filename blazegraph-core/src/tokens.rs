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
pub fn estimate_token_count(text: &str) -> usize {
    text.len() / 4 // Rough estimation: ~4 characters per token
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
}
