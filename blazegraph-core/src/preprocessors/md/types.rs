//! Public API surface for markdown parsing (`preprocessors::md`).
//!
//! `ParseOptions` controls behavior (strict vs. drift-accepting),
//! `ParseResult` carries the reconstructed graph + a parse-time identity
//! signal, `ParseIdentity` is that signal, and `ParseError` is the
//! typed error enum.
//!
//! Wire-format definition: `docs/P2/core/architecture/08-bgraph-md-format.md`.

use crate::types::DocumentGraph;

/// Options controlling markdown parse behavior.
///
/// `accept_drift = false` (the default) is **strict mode**: if the
/// `graph_sha256` recomputed from the parsed graph does not match the
/// value embedded in the doc-level block, `parse` returns
/// `Err(ParseError::HashMismatch { .. })`. This is the right setting
/// for round-trip verification (B4) and for any pipeline that wants
/// byte-identical reproduction.
///
/// `accept_drift = true` lets the parser tolerate a hash mismatch and
/// return `ParseResult { identity: ParseIdentity::Derivative { .. },
/// .. }` — the graph still parses, but the caller now knows it is
/// *derived from* (not identical to) the embedded `graph_sha256`. Use
/// this when round-tripping a hand-edited bgraph.md.
#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    /// If true, `graph_sha256` mismatch returns
    /// `ParseIdentity::Derivative` instead of erroring.
    /// Default: false (strict mode).
    pub accept_drift: bool,
}

/// Result of a successful parse.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The reconstructed graph.
    pub graph: DocumentGraph,
    /// Round-trip identity status for this parse.
    pub identity: ParseIdentity,
}

/// Round-trip identity status for a parsed graph.
///
/// `Verified` means `graph_sha256(parsed_graph) ==
/// doc_level.graph_sha256` — the parsed graph is bit-for-bit the
/// original. `Derivative` carries both hashes so consumers can record
/// "this graph is derived from {original_sha256}" in their own
/// provenance metadata; the graph itself does not carry drift state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseIdentity {
    /// `graph_sha256` recomputed from the parsed graph matched the
    /// value embedded in the doc-level block. The graph is bit-for-bit
    /// the original.
    Verified,
    /// `graph_sha256` did not match. Only returned when
    /// `ParseOptions.accept_drift = true`. Carries both hashes for
    /// provenance.
    Derivative {
        /// The `graph_sha256` value embedded in the source markdown's
        /// doc-level block.
        original_sha256: String,
        /// The `graph_sha256` recomputed from the parsed graph.
        recomputed_sha256: String,
    },
}

/// Strip mode for the [`crate::preprocessors::md::strip`] operation.
///
/// Three variants per the spec's [Strip ergonomics] table — one for
/// each of the sed patterns the v1.0.0 wire format was designed to be
/// compatible with. The dash discriminator (`bgraph-[a-z-]*` vs.
/// `bgraph[a-z-]*`) is load-bearing, hence the three named modes
/// rather than a single boolean.
///
/// [Strip ergonomics]:
/// https://github.com/AmplifyTechnology/blazegraph-io-app/blob/main/docs/P2/core/architecture/08-bgraph-md-format.md#strip-ergonomics
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripMode {
    /// Remove all bgraph fences (doc-level + bookmarks + every
    /// per-element fence, including Header/Footer/Margin inside-fence
    /// bodies). Section/Paragraph body text survives.
    ///
    /// Equivalent to `sed -E '/^```bgraph[a-z-]*$/,/^```$/d'`.
    /// Output is Unstructured-equivalent body-only prose suitable for
    /// AI tools / RAG ingestion.
    BodyOnly,
    /// Keep the doc-level `bgraph` block. Strip per-element fences
    /// (and any inside-fence body for Header/Footer/Margin), but
    /// preserve Section/Paragraph body text.
    ///
    /// Equivalent to `sed -E '/^```bgraph-[a-z-]*$/,/^```$/d'`.
    /// Useful when you want plain prose + provenance + graph identity.
    KeepMetadata,
    /// Strip only Header/Footer/Margin fences (and their inside-fence
    /// bodies). Doc-level block, bookmarks, and Section/Paragraph
    /// fences + bodies all survive.
    ///
    /// Equivalent to
    /// `sed -E '/^```bgraph-(header|footer|margin)$/,/^```$/d'`.
    /// Removes running-noise (page headers/footers/margins) while
    /// preserving full structural metadata.
    NoiseOnly,
}

/// Errors from markdown parsing.
///
/// `HashMismatch` is the load-bearing variant for B5 (CLI). Other
/// variants are surfaced for fail-loud parser-bug detection.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The input is not bgraph.md (no leading `bgraph` fence detected)
    /// and the generic-markdown ingestion path is not yet implemented.
    /// v1 scope is the round-trip artifact only.
    #[error("input is not bgraph.md and generic markdown is not yet supported")]
    GenericMarkdownNotYetSupported,

    /// The first fence in the input was not a `bgraph` doc-level
    /// block, or the doc-level block JSON failed to parse.
    #[error("missing or malformed document-level bgraph block")]
    MissingDocLevelBlock,

    /// A bgraph fence appeared in an invalid position or with an
    /// invalid shape (e.g., `bgraph-bookmarks` not immediately after
    /// the doc-level block, or a Header/Footer/Margin fence with no
    /// body content).
    #[error("malformed bgraph fence: {0}")]
    MalformedFence(String),

    /// JSON parsing failed inside a bgraph fence (doc-level,
    /// bookmarks, or per-element).
    #[error("invalid JSON in bgraph fence: {source}")]
    JsonParse {
        #[source]
        source: serde_json::Error,
    },

    /// `graph_sha256` recomputed from the parsed graph did not match
    /// the value embedded in the doc-level block, and strict mode
    /// (`ParseOptions.accept_drift = false`) was requested.
    #[error("graph_sha256 mismatch: original={original}, recomputed={recomputed}")]
    HashMismatch {
        original: String,
        recomputed: String,
    },

    /// The doc-level block carried a `schema` field whose major
    /// version is not `1`.
    #[error("unsupported schema version {0}; expected 1.0.x")]
    UnsupportedSchema(String),

    /// Body content contained a line starting with the reserved
    /// `` ```bgraph `` prefix that does not match a recognized fence
    /// tag (or appears mid-body where no fence is allowed). The v1.0.0
    /// spec reserves this prefix at line-start.
    #[error("body content contains reserved prefix '```bgraph' at line-start")]
    ReservedPrefixInBody,

    /// A per-element fence carried a `node_type` that is not one of
    /// the v1.0.0 element types (`Section`, `Paragraph`, `Header`,
    /// `Footer`, `Margin`).
    #[error("unknown node_type {0:?}; expected Section/Paragraph/Header/Footer/Margin")]
    UnknownNodeType(String),

    /// A per-element fence carried an `id` that did not match the ID
    /// the `NodeIdGenerator` would have derived from the parsed
    /// provenance triple + `text_order`. Indicates the markdown was
    /// emitted by a different (version, source, config) than its
    /// metadata claims, or was hand-tampered.
    #[error(
        "per-element id mismatch at text_order {text_order}: parsed={parsed}, expected={expected}"
    )]
    IdMismatch {
        text_order: u32,
        parsed: String,
        expected: String,
    },
}
