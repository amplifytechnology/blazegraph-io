//! Markdown preprocessor (`preprocessors::md`).
//!
//! This module is the ingestion-side counterpart to the bgraph.md
//! emitter at `crate::graphs::serialization::markdown`. Together they
//! close the round-trip loop for the bgraph.md wire format
//! (`docs/P2/core/architecture/08-bgraph-md-format.md`):
//!
//! ```text
//!     DocumentGraph ──emit_markdown──▶ bgraph.md string
//!           ▲                                │
//!           └────── parse_markdown ──────────┘
//! ```
//!
//! ## Why this does not implement the `Preprocessor` trait
//!
//! The bgraph.md path is *special*: a bgraph.md string is already a
//! fully-formed graph projection — it carries deterministic IDs,
//! provenance, bookmarks, and metadata in its document-level block.
//! Parsing it goes directly to a `DocumentGraph`, skipping the
//! `PreprocessorOutput` shape that the `Preprocessor` trait produces.
//!
//! Future generic-markdown ingestion (no bgraph metadata, just prose
//! with `#` headings) would implement the trait the same way
//! `preprocessors::pdf` does — but that work is deferred. v1 scope is
//! the round-trip artifact only. See
//! `ParseError::GenericMarkdownNotYetSupported`.

pub mod bgraph_md;
pub mod frontmatter;
pub mod generic_md;
pub mod strip;
pub mod types;

pub use strip::strip;
pub use types::{ParseError, ParseIdentity, ParseOptions, ParseResult, StripMode};

/// Parse a markdown string into a `DocumentGraph`.
///
/// Auto-detects the markdown variant:
/// - bgraph.md (round-trip artifact emitted by the B2 forward emitter)
///   → full reconstruction via [`bgraph_md::parse`].
/// - Generic markdown (no bgraph metadata) → not yet supported;
///   returns [`ParseError::GenericMarkdownNotYetSupported`].
///
/// Callers who already know the input is bgraph.md can skip detection
/// by calling [`bgraph_md::parse`] directly.
pub fn parse_markdown(input: &str, opts: ParseOptions) -> Result<ParseResult, ParseError> {
    if is_bgraph_md(input) {
        bgraph_md::parse(input, opts)
    } else {
        Err(ParseError::GenericMarkdownNotYetSupported)
    }
}

/// Sniff the input to detect the bgraph.md variant.
///
/// Heuristic: the first non-blank line is literally ` ```bgraph ` (no
/// suffix), AND the next line parses as JSON containing both `schema`
/// and `graph_sha256` keys. Cheap; the false-positive risk is
/// negligible because the prefix is reserved by the v1.0.0 spec
/// (see "Reserved fence prefix" in
/// `docs/P2/core/architecture/08-bgraph-md-format.md`).
pub fn is_bgraph_md(input: &str) -> bool {
    let mut lines = input.lines().skip_while(|l| l.trim().is_empty());
    let Some(first) = lines.next() else {
        return false;
    };
    if first.trim_end() != "```bgraph" {
        return false;
    }
    let Some(json_line) = lines.next() else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json_line) else {
        return false;
    };
    let Some(obj) = value.as_object() else {
        return false;
    };
    obj.contains_key("schema") && obj.contains_key("graph_sha256")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal hand-crafted bgraph.md good enough for the sniffer to
    /// accept (the parser may reject for other reasons; this is a
    /// detection test, not a reconstruction test).
    fn sample_bgraph_md_header() -> &'static str {
        "```bgraph\n\
         {\"schema\":\"1.0.0\",\"blazegraph_version\":\"0.6.0\",\"source\":{\"format\":\"pdf\",\"filename\":\"x.pdf\",\"sha256\":\"abc\"},\"flow_type\":\"Fixed\",\"title\":null,\"config_hash\":\"def\",\"graph_sha256\":\"deadbeef\"}\n\
         ```\n"
    }

    #[test]
    fn is_bgraph_md_returns_true_for_emitter_output() {
        // Build a graph and emit through the real emitter; sniffer
        // must accept it.
        use crate::graphs::serialization::markdown::emit_markdown;
        use crate::types::*;
        use std::collections::HashMap;
        use uuid::Uuid;

        let root_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"sniff-root");
        let para_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"sniff-0");
        let mut nodes = HashMap::new();
        nodes.insert(
            root_id,
            DocumentNode {
                id: root_id,
                node_type: "Document".to_string(),
                location: NodeLocation {
                    semantic: SemanticLocation {
                        path: String::new(),
                        depth: 0,
                        breadcrumbs: Vec::new(),
                    },
                    physical: None,
                },
                text_order: None,
                content: NodeContent {
                    text: "Document".to_string(),
                },
                style_info: None,
                token_count: 0,
                parent: None,
                children: vec![para_id],
            },
        );
        nodes.insert(
            para_id,
            DocumentNode {
                id: para_id,
                node_type: "Paragraph".to_string(),
                location: NodeLocation {
                    semantic: SemanticLocation {
                        path: "1".to_string(),
                        depth: 1,
                        breadcrumbs: Vec::new(),
                    },
                    physical: None,
                },
                text_order: Some(0),
                content: NodeContent {
                    text: "Body.".to_string(),
                },
                style_info: None,
                token_count: 1,
                parent: Some(root_id),
                children: Vec::new(),
            },
        );
        let graph = DocumentGraph {
            nodes,
            document_info: DocumentInfo {
                root_id,
                document_metadata: DocumentMetadata::default(),
                bookmark_data: None,
                parse_provenance: Some(ParseProvenance {
                    blazegraph_version: "0.6.0".to_string(),
                    source_format: "markdown".to_string(),
                    source_filename: "x.md".to_string(),
                    source_sha256: "abc".to_string(),
                    config_hash: "def".to_string(),
                }),
            },
            structural_profile: StructuralProfile::default(),
        };
        let md = emit_markdown(&graph);
        assert!(
            is_bgraph_md(&md),
            "emitter output should sniff as bgraph.md"
        );
    }

    #[test]
    fn is_bgraph_md_returns_false_for_plain_markdown() {
        let md = "# Title\n\nPlain prose.\n";
        assert!(!is_bgraph_md(md));
    }

    #[test]
    fn is_bgraph_md_returns_false_for_empty_input() {
        assert!(!is_bgraph_md(""));
        assert!(!is_bgraph_md("\n\n\n"));
    }

    #[test]
    fn is_bgraph_md_skips_leading_blank_lines() {
        let mut input = String::from("\n\n");
        input.push_str(sample_bgraph_md_header());
        assert!(is_bgraph_md(&input));
    }

    #[test]
    fn is_bgraph_md_returns_false_when_first_line_is_wrong() {
        // Looks bgraph-ish but isn't the exact prefix.
        let bad = "```bgraph-section\n{\"id\":\"x\"}\n```\n";
        assert!(!is_bgraph_md(bad));
    }

    #[test]
    fn is_bgraph_md_returns_false_when_json_line_missing_required_keys() {
        let bad = "```bgraph\n{\"hello\":\"world\"}\n```\n";
        assert!(!is_bgraph_md(bad));
    }

    #[test]
    fn parse_markdown_dispatches_bgraph_md_to_reverse_parser() {
        // We don't need the parse to succeed here — just that the
        // dispatch goes to bgraph_md::parse, which will return
        // something *other than* GenericMarkdownNotYetSupported.
        let input = sample_bgraph_md_header();
        let result = parse_markdown(input, ParseOptions::default());
        assert!(
            !matches!(result, Err(ParseError::GenericMarkdownNotYetSupported)),
            "bgraph.md input should not return GenericMarkdownNotYetSupported",
        );
    }

    #[test]
    fn parse_markdown_returns_generic_not_yet_supported_for_plain_md() {
        let input = "# Title\n\nSome prose.\n";
        let result = parse_markdown(input, ParseOptions::default());
        assert!(matches!(
            result,
            Err(ParseError::GenericMarkdownNotYetSupported)
        ));
    }
}
