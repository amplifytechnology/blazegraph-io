//! The versioned-codec seam (arch-14 §7 live tier; museum design-flow
//! Block C).
//!
//! This module is the single place where "which version's canonical
//! form" becomes an *explicit input* rather than an implicit hardcoded
//! const. Today it has exactly **one arm** ([`FormatVersion::V1_0`], the
//! honest inaugural edition of the content-body-identity substrate). The
//! point is not the machinery — with one arm the dispatch is trivial —
//! but the *seam's existence*: the next real format change adds an arm
//! here (a new canonicalization profile / codec) instead of forcing a
//! refactor of the two hardcoded stamp sites.
//!
//! Three entry points make the version an argument:
//!
//! - [`FormatVersion::from_schema_str`] — read-side recognition. Replaces
//!   the string-prefix logic that used to live inline in
//!   `bgraph_md::validate_schema`: a `1.x` schema maps to the current
//!   arm; anything else is unrecognized (→ `UnsupportedSchema`).
//! - [`canonicalize_as`] / [`emit_markdown_as`] — write-side dispatch,
//!   wrapping today's `canonical_json` and `emit_markdown_with_options`.
//! - [`upcast`] — the migration hook. A no-op for the current version by
//!   design; future migrations add the real body here.
//!
//! The two hardcoded stamp sites (`markdown.rs::emit_document_level_block`
//! and `graph.rs::to_sorted_graph`) now stamp
//! [`FormatVersion::CURRENT`]`.schema_str()`, not a bare const — so the
//! version threaded into the artifact and the version the read path
//! recognizes are the *same* enum, not two coincidentally-equal strings.

use crate::graphs::serialization::canonical;
use crate::graphs::serialization::markdown::{emit_markdown_with_options, EmitOptions};
use crate::preprocessors::md::BGRAPH_FORMAT_VERSION;
use crate::types::{DocumentGraph, ParseProvenance};

/// The format version of a bgraph artifact — the schema/format axis of
/// the version model (arch-15), reified as an enum so version handling is
/// an explicit dispatch instead of a string-prefix match.
///
/// **One arm today.** `V1_0` is the current (and only) edition. A future
/// format change that cannot be expressed as an additive/rename profile
/// is a major bump (arch-14 §7) and adds a variant here in lock-step with
/// a new canonicalization profile + a read-path arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatVersion {
    /// `1.x` — the honest inaugural edition of the content-body-identity
    /// substrate (Block C reset; was the pre-museum `5.x` churn). One
    /// structural read path, one canonical form.
    V1_0,
}

impl FormatVersion {
    /// The version new artifacts are emitted as. Every write-side stamp
    /// site routes through this designation rather than a bare const, so
    /// "the current edition" is named once.
    pub const CURRENT: FormatVersion = FormatVersion::V1_0;

    /// The schema string stamped into the artifact for this version
    /// (the bgraph.md doc-level `schema` field / the json wrapper's
    /// `schema_version`). For [`FormatVersion::V1_0`] this is exactly
    /// [`BGRAPH_FORMAT_VERSION`] (`"1.0.0"`) — keeping the enum and the
    /// const in sync so the byte-identical-emit contract holds.
    pub fn schema_str(self) -> &'static str {
        match self {
            FormatVersion::V1_0 => BGRAPH_FORMAT_VERSION,
        }
    }

    /// Recognize a schema string on the read path. A `1.x` string maps to
    /// the current arm; anything else is unrecognized (`None`), which the
    /// caller turns into `ParseError::UnsupportedSchema`. This is the
    /// single read-side version chokepoint (was the inline prefix match in
    /// `bgraph_md::validate_schema`).
    ///
    /// The `1→5` history is deliberately *not* accepted: those are
    /// internal pre-museum editions with no external consumer (the "no
    /// fictional users" principle), so a non-`1.x` file is a clean
    /// `Unsupported`, never something to best-effort-read.
    pub fn from_schema_str(schema: &str) -> Option<FormatVersion> {
        if schema.starts_with("1.") {
            Some(FormatVersion::V1_0)
        } else {
            None
        }
    }
}

/// Canonicalize a graph *as* the given format version — the seam wrapping
/// [`canonical::canonical_json`]. For the current version this is the
/// identity canonicalizer; a future arm supplies that version's frozen
/// canonicalization profile (arch-14 §7) so an old edition still hashes to
/// its stamped `graph_sha256`.
pub fn canonicalize_as(version: FormatVersion, graph: &DocumentGraph) -> String {
    match version {
        FormatVersion::V1_0 => canonical::canonical_json(graph),
    }
}

/// Emit a graph to bgraph.md *as* the given format version — the seam
/// wrapping [`emit_markdown_with_options`]. One arm today; a future arm
/// would dispatch to that version's frozen emitter.
pub fn emit_markdown_as(
    version: FormatVersion,
    graph: &DocumentGraph,
    provenance: &ParseProvenance,
    opts: EmitOptions,
) -> String {
    match version {
        FormatVersion::V1_0 => emit_markdown_with_options(graph, provenance, opts),
    }
}

/// The migration hook: upcast a graph parsed under `from` to the current
/// in-memory shape. **A no-op today by design** — with a single codec arm
/// there is nothing to migrate; every readable file is already the
/// current version. This is the seam for future cross-version migration
/// (arch-14 §7.1): when a later edition changes the graph shape, its arm
/// here performs the invertible upcast so the rest of the pipeline only
/// ever sees the current struct.
pub fn upcast(graph: DocumentGraph, from: FormatVersion) -> DocumentGraph {
    match from {
        FormatVersion::V1_0 => graph,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_is_v1_0_and_stamps_the_const() {
        assert_eq!(FormatVersion::CURRENT, FormatVersion::V1_0);
        assert_eq!(FormatVersion::CURRENT.schema_str(), BGRAPH_FORMAT_VERSION);
        assert_eq!(FormatVersion::CURRENT.schema_str(), "1.0.0");
    }

    #[test]
    fn from_schema_str_accepts_only_1_x() {
        for ok in ["1.0.0", "1.1.0", "1.42.7"] {
            assert_eq!(
                FormatVersion::from_schema_str(ok),
                Some(FormatVersion::V1_0),
                "{ok} should map to the current arm"
            );
        }
        for bad in ["0.9.0", "2.0.0", "5.0.0", "6.0.0", "10.0.0"] {
            assert_eq!(
                FormatVersion::from_schema_str(bad),
                None,
                "{bad} should be unrecognized"
            );
        }
    }

    #[test]
    fn canonicalize_as_current_equals_canonical_json() {
        let graph = DocumentGraph::new();
        assert_eq!(
            canonicalize_as(FormatVersion::CURRENT, &graph),
            canonical::canonical_json(&graph),
            "the V1_0 arm must be the identity canonicalizer (pure refactor)"
        );
    }

    #[test]
    fn upcast_current_is_noop() {
        let graph = DocumentGraph::new();
        let before = canonical::graph_sha256(&graph);
        let after = upcast(graph, FormatVersion::V1_0);
        assert_eq!(
            before,
            canonical::graph_sha256(&after),
            "upcast of the current version must not touch the graph"
        );
    }
}
