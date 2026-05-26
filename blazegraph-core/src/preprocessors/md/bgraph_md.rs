//! bgraph.md reverse parser — string → `DocumentGraph`.
//!
//! Mirror of the forward emitter at
//! `crate::graphs::serialization::markdown`. Wire-format spec at
//! `docs/P2/core/architecture/08-bgraph-md-format.md` is the source of
//! truth; this module implements the parser side of the round-trip.
//!
//! **Single-convention parser (CR-57 / v2.1.0+).** Accepts only the
//! v2.1.0 wire format: kebab-case variant tags (`bgraph-code-block`,
//! `bgraph-block-quote`), the `bgraph-metadata` fence carrying the full
//! `DocumentMetadata`, and the doc-level `bgraph` block without `title`.
//! Per [no_fictional_users](feedback_no_fictional_users.md) + CR-56 § I.5,
//! there are no live v2.0.0 consumers — dual-accept is overhead without
//! a beneficiary. v1.x and v2.0.0 fixtures regenerate via the emit path.
//!
//! **Dogfooding (CR-56 § I.7).** This parser uses only the structural
//! walk + `serde_json` — no markdown AST library (no `pulldown-cmark`,
//! no `comrak`). Format-drift that breaks the walk breaks `bgraph_md.rs`
//! first; downstream consumers cannot drift undetected.

use crate::graphs::builder::GraphBuilder;
use crate::graphs::node_id::NodeIdGenerator;
use crate::graphs::serialization::canonical;
use crate::types::*;
use serde::Deserialize;

use super::types::{ParseError, ParseIdentity, ParseOptions, ParseResult};

/// Parse a bgraph.md string into a `DocumentGraph`.
///
/// Callers that haven't already verified the input is bgraph.md should
/// use [`super::parse_markdown`] instead (it sniffs first and dispatches).
///
/// On success returns the reconstructed graph plus a parse-time
/// identity signal:
/// - [`ParseIdentity::Verified`] if `graph_sha256` of the parsed graph
///   matches the value embedded in the doc-level block.
/// - [`ParseIdentity::Derivative`] if it does not match and
///   `opts.accept_drift = true`.
///
/// On hash mismatch with `opts.accept_drift = false` (the default),
/// returns [`ParseError::HashMismatch`].
pub fn parse(input: &str, opts: ParseOptions) -> Result<ParseResult, ParseError> {
    // ----- Phase 1: extract fence regions and free body chunks. -----
    let segments = scan_segments(input)?;

    // ----- Phase 2: classify segments into doc-level block,
    // bgraph-metadata, bookmarks, and per-element fences. -----
    let mut doc_level: Option<DocLevelBlock> = None;
    let mut document_metadata: DocumentMetadata = DocumentMetadata::default();
    let mut metadata_seen = false;
    let mut bookmarks: Option<BookmarkData> = None;
    let mut parsed_elements: Vec<ParsedElement> = Vec::new();

    // Body content cache: the most recent free-line block (raw text
    // outside any fence) becomes the body for the next
    // Section / Paragraph fence we encounter.
    let mut pending_body: Option<String> = None;

    for seg in &segments {
        match seg {
            Segment::FreeBlock { text } => {
                pending_body = Some(text.clone());
            }
            Segment::Fence { tag, body } => {
                match tag.as_str() {
                    "bgraph" => {
                        if doc_level.is_some() {
                            return Err(ParseError::MalformedFence(
                                "duplicate doc-level bgraph fence".to_string(),
                            ));
                        }
                        if metadata_seen
                            || !parsed_elements.is_empty()
                            || bookmarks.is_some()
                        {
                            return Err(ParseError::MalformedFence(
                                "doc-level bgraph fence must appear first".to_string(),
                            ));
                        }
                        let parsed: DocLevelBlock =
                            serde_json::from_str(body.trim_end_matches('\n'))
                                .map_err(|source| ParseError::JsonParse { source })?;
                        validate_schema(&parsed.schema)?;
                        doc_level = Some(parsed);
                        pending_body = None;
                    }
                    "bgraph-metadata" => {
                        // CR-56 § I.3: doc-extracted DocumentMetadata
                        // fence. Required by the v2.1.0+ emitter but
                        // tolerated as absent in hand-crafted inputs
                        // (the walk algorithm makes unknown / missing
                        // fences a no-op; default DocumentMetadata is a
                        // valid value).
                        if doc_level.is_none() {
                            return Err(ParseError::MissingDocLevelBlock);
                        }
                        if metadata_seen {
                            return Err(ParseError::MalformedFence(
                                "duplicate bgraph-metadata fence".to_string(),
                            ));
                        }
                        if bookmarks.is_some() || !parsed_elements.is_empty() {
                            return Err(ParseError::MalformedFence(
                                "bgraph-metadata fence must appear before bgraph-bookmarks \
                                 and per-element fences"
                                    .to_string(),
                            ));
                        }
                        metadata_seen = true;
                        document_metadata =
                            serde_json::from_str(body.trim_end_matches('\n'))
                                .map_err(|source| ParseError::JsonParse { source })?;
                        pending_body = None;
                    }
                    "bgraph-bookmarks" => {
                        if doc_level.is_none() {
                            return Err(ParseError::MissingDocLevelBlock);
                        }
                        if bookmarks.is_some() {
                            return Err(ParseError::MalformedFence(
                                "duplicate bgraph-bookmarks fence".to_string(),
                            ));
                        }
                        if !parsed_elements.is_empty() {
                            return Err(ParseError::MalformedFence(
                                "bgraph-bookmarks fence must appear before per-element fences"
                                    .to_string(),
                            ));
                        }
                        let bm: BookmarkData = serde_json::from_str(body.trim_end_matches('\n'))
                            .map_err(|source| ParseError::JsonParse { source })?;
                        bookmarks = Some(bm);
                        pending_body = None;
                    }
                    "bgraph-section" => {
                        if doc_level.is_none() {
                            return Err(ParseError::MissingDocLevelBlock);
                        }
                        let heading_body = pending_body.take().ok_or_else(|| {
                            ParseError::MalformedFence(
                                "bgraph-section fence with no preceding heading line".to_string(),
                            )
                        })?;
                        let text = strip_heading_prefix(&heading_body);
                        let meta = parse_node_metadata(body)?;
                        parsed_elements.push(ParsedElement {
                            body: text,
                            metadata: meta,
                        });
                    }
                    "bgraph-paragraph"
                    | "bgraph-header"
                    | "bgraph-footer"
                    | "bgraph-margin"
                    | "bgraph-code-block"
                    | "bgraph-list"
                    | "bgraph-block-quote"
                    | "bgraph-table" => {
                        // v2.1.0+ single convention: body-outside for
                        // every content variant (including H/F/M; CR-48
                        // / Amendment H) and kebab-case for multi-word
                        // tags (F-11 / CR-56 § I.5).
                        if doc_level.is_none() {
                            return Err(ParseError::MissingDocLevelBlock);
                        }
                        let body_text = pending_body.take().ok_or_else(|| {
                            ParseError::MalformedFence(format!(
                                "{tag} fence with no preceding body line",
                            ))
                        })?;
                        let meta = parse_node_metadata(body)?;
                        parsed_elements.push(ParsedElement {
                            body: body_text.trim().to_string(),
                            metadata: meta,
                        });
                    }
                    other => {
                        return Err(ParseError::MalformedFence(format!(
                            "unrecognized bgraph fence tag: {other:?}"
                        )));
                    }
                }
            }
        }
    }

    let doc_level = doc_level.ok_or(ParseError::MissingDocLevelBlock)?;

    // ----- Phase 3: reconstruct the graph. -----
    // Sort by text_order (defensive — should already be ascending).
    parsed_elements.sort_by_key(|p| p.metadata.text_order);

    // Build the SemanticTreeElement vec the GraphBuilder consumes.
    let mut semantic_elements: Vec<SemanticTreeElement> = Vec::with_capacity(parsed_elements.len());
    for (i, p) in parsed_elements.iter().enumerate() {
        debug_assert_eq!(
            i as u32, p.metadata.text_order,
            "text_order drift at index {i}: parsed text_order = {}",
            p.metadata.text_order,
        );
        let element_type = map_node_type_to_semantic(&p.metadata.node_type)?;
        // For Header/Footer/Margin/Paragraph the depth carried in
        // metadata is the *post-build* tree depth — the depth assigned
        // by GraphBuilder's find_parent algorithm based on the
        // hierarchy_level we pass in. The hierarchy_level we set here
        // determines find_parent's parent selection: Section nodes use
        // their level explicitly; non-Section nodes pass through the
        // builder's "leaves attached to the current open Section" rule
        // when their level is 0, but the emitter wrote a non-zero depth
        // on these nodes (inherited from their containing Section).
        // Passing depth as hierarchy_level preserves the builder's
        // stack-attach behavior because non-Section paths are leaves —
        // find_parent only consults hierarchy_level for Section
        // re-stacking.
        let hierarchy_level = match element_type {
            SemanticElementType::Section => p.metadata.location.semantic.depth,
            _ => p.metadata.location.semantic.depth,
        };
        semantic_elements.push(
            SemanticTreeElement {
                text: p.body.clone(),
                element_type,
                hierarchy_level,
                text_order: p.metadata.text_order,
                physical_location: p.metadata.location.physical.clone(),
                // CR-45 (v2.1.0+): per-element `style` is a first-class field
                // in the bgraph fence's JSON. The graph builder copies
                // `element.style` onto `DocumentNode.style_info`, restoring
                // round-trip identity for PDF-source graphs that carry style.
                // CR-59 (v2.1.0+): emission is gated by
                // `EmitOptions::include_style_info`; the parser still tolerates
                // the field on inputs that carry it.
                style: p.metadata.style.clone(),
                token_count: p.metadata.token_count,
            }
            .validate(),
        );
    }

    // NodeIdGenerator from doc-level provenance. CR-47 dropped
    // `blazegraph_version` from the namespace, so node IDs are
    // already version-invariant — the parser side and the emitter
    // side derive identical IDs from `(source_sha256, config_hash)`
    // regardless of which blazegraph version ran which step. The
    // `blazegraph_version` field in `doc_level` stays as provenance
    // documentation only (see CR-47 Amendment G).
    let id_gen = NodeIdGenerator::new(&doc_level.source.sha256, &doc_level.config_hash);

    let provenance = ParseProvenance {
        blazegraph_version: doc_level.blazegraph_version.clone(),
        source_format: doc_level.source.format.clone(),
        source_filename: doc_level.source.filename.clone(),
        source_sha256: doc_level.source.sha256.clone(),
        config_hash: doc_level.config_hash.clone(),
    };

    let mut graph = GraphBuilder::new()
        .build_graph_deterministic(semantic_elements, &id_gen, provenance)
        .map_err(|e| ParseError::MalformedFence(format!("graph build failed: {e}")))?;

    // ----- Phase 4: populate fields the builder does not. -----
    // CR-57 / v2.1.0: document_metadata comes from the bgraph-metadata
    // fence (full DocumentMetadata, canonical + namespaced) — not the
    // doc-level `bgraph` block. When the fence is absent (hand-crafted
    // inputs), `document_metadata` defaults to `Default::default()`,
    // which is the honest "no extracted metadata" answer.
    graph.document_info.document_metadata = document_metadata;
    graph.document_info.bookmark_data = bookmarks;
    // CR-49 (v2.1.0+) added `topology` here.
    // CR-60 (2026-05-22) retracted `source_identity` + `supersedes`
    // per the byte-in/byte-out principle (arch doc 11 + DT-04).
    // Absent in the source bgraph.md → `None` on the reconstructed graph.
    graph.document_info.topology = doc_level.topology.clone();
    graph.structural_profile.flow_type = doc_level.flow_type.clone();

    // Canonical post-build sequence (mirrors processor.rs:501-502).
    graph.compute_structural_profile();
    graph.compute_breadcrumbs();

    // Self-consistency check (debug only): verify the IDs the builder
    // derived match the IDs the emitter embedded. A mismatch here means
    // the parsed bgraph.md was emitted by a different
    // (version, source, config) than its metadata claims, or was
    // hand-tampered. Non-load-bearing for round-trip identity (the
    // builder's IDs are the truth), but a useful fail-loud signal.
    #[cfg(debug_assertions)]
    {
        for p in &parsed_elements {
            let derived = id_gen.node_id(p.metadata.text_order);
            debug_assert_eq!(
                derived, p.metadata.id,
                "per-element id mismatch at text_order {}: parsed={}, derived={}",
                p.metadata.text_order, p.metadata.id, derived
            );
        }
    }

    // ----- Phase 5: identity verification. -----
    let recomputed = canonical::graph_sha256(&graph);
    let identity = if recomputed == doc_level.graph_sha256 {
        ParseIdentity::Verified
    } else if opts.accept_drift {
        ParseIdentity::Derivative {
            original_sha256: doc_level.graph_sha256.clone(),
            recomputed_sha256: recomputed,
        }
    } else {
        return Err(ParseError::HashMismatch {
            original: doc_level.graph_sha256,
            recomputed,
        });
    };

    Ok(ParseResult { graph, identity })
}

// =========================================================================
// Internal: line-scan into segments.
// =========================================================================

/// One segment of parsed input: either a free-text block (markdown
/// content between fences) or a bgraph fence with its tag + inner body.
#[derive(Debug)]
enum Segment {
    /// Raw text content between bgraph fences. Newlines preserved
    /// verbatim. May be empty / whitespace-only (in which case it is
    /// discarded by the caller).
    FreeBlock { text: String },
    /// A bgraph fence: tag is `bgraph` or `bgraph-*`; body is the raw
    /// content between the fence open and close lines (with the
    /// trailing newline preserved if any, but without the open/close
    /// fence lines themselves).
    Fence { tag: String, body: String },
}

/// Walk the input line by line, slicing it into `Segment`s.
///
/// A bgraph fence open is a line whose trimmed-end form is exactly
/// ` ```bgraph ` or ` ```bgraph-* `. The fence close is the next line
/// whose trimmed-end form is ` ``` ` (no info string).
///
/// Lines outside any fence accumulate into a `FreeBlock`; blank-only
/// runs are kept verbatim (the caller decides if they matter).
fn scan_segments(input: &str) -> Result<Vec<Segment>, ParseError> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut free_buf: Vec<&str> = Vec::new();
    let mut iter = input.split_inclusive('\n').peekable();

    while let Some(line) = iter.next() {
        let trimmed_line = strip_trailing_newline(line);
        if let Some(tag) = bgraph_fence_open_tag(trimmed_line) {
            // Flush free buffer first.
            if !free_buf.is_empty() {
                let text = collapse_free_buffer(&free_buf);
                if !text.trim().is_empty() {
                    segments.push(Segment::FreeBlock { text });
                }
                free_buf.clear();
            }
            // Collect body up to the next ` ``` ` close.
            let mut body = String::new();
            let mut closed = false;
            for body_line in iter.by_ref() {
                let body_trimmed = strip_trailing_newline(body_line);
                if is_bare_fence_close(body_trimmed) {
                    closed = true;
                    break;
                }
                // Defensive: catch reserved prefix in body — if a body
                // line opens a *new* bgraph fence before we see the
                // close, the format is malformed.
                if bgraph_fence_open_tag(body_trimmed).is_some() {
                    return Err(ParseError::ReservedPrefixInBody);
                }
                body.push_str(body_line);
            }
            if !closed {
                return Err(ParseError::MalformedFence(format!(
                    "unterminated bgraph fence with tag {tag:?}"
                )));
            }
            segments.push(Segment::Fence { tag, body });
        } else {
            free_buf.push(line);
        }
    }
    // Trailing free block (e.g., a final newline after the last fence
    // close). Almost always whitespace-only; discard if so.
    if !free_buf.is_empty() {
        let text = collapse_free_buffer(&free_buf);
        if !text.trim().is_empty() {
            segments.push(Segment::FreeBlock { text });
        }
    }
    Ok(segments)
}

/// If `line` is a bgraph fence open, return the tag (without the
/// leading triple-backticks). Otherwise `None`.
///
/// Recognized tags (v2.1.0+ / CR-57): `bgraph`, `bgraph-metadata`,
/// `bgraph-bookmarks`, `bgraph-section`, `bgraph-paragraph`,
/// `bgraph-header`, `bgraph-footer`, `bgraph-margin`, `bgraph-code-block`,
/// `bgraph-list`, `bgraph-block-quote`, `bgraph-table`. Any other
/// ` ```bgraph* ` line-start is rejected by the caller as a
/// reserved-prefix violation.
///
/// CR-59 retracted `bgraph-message` (added by CR-49); the variant has
/// no in-memory carrier in tree-topology code paths. Inputs containing
/// `bgraph-message` now fall through to the reserved-prefix-violation
/// path.
///
/// Visible at `pub(super)` so the sibling [`super::strip`] module can
/// reuse the same fence-recognition rules without re-implementing them.
pub(super) fn bgraph_fence_open_tag(line: &str) -> Option<String> {
    let info = line.strip_prefix("```")?;
    if info == "bgraph"
        || info == "bgraph-metadata"
        || info == "bgraph-bookmarks"
        || info == "bgraph-section"
        || info == "bgraph-paragraph"
        || info == "bgraph-header"
        || info == "bgraph-footer"
        || info == "bgraph-margin"
        || info == "bgraph-code-block"
        || info == "bgraph-list"
        || info == "bgraph-block-quote"
        || info == "bgraph-table"
    {
        Some(info.to_string())
    } else if let Some(rest) = info.strip_prefix("bgraph") {
        // Anything starting with `bgraph` at line-start that isn't a
        // recognized v2.1.0 tag is a reserved-prefix violation (per
        // spec § Reserved fence prefix). The caller (scan_segments)
        // treats `None` as "not a fence" — flag this case explicitly so
        // the outer match returns ReservedPrefixInBody (or surfaces an
        // unrecognized-tag error if the tag dispatch reaches it).
        let _ = rest; // silence unused warning when no suffixes left
        Some(format!("bgraph{rest}"))
    } else {
        None
    }
}

/// Trim the trailing `\n` (and optional preceding `\r`) from a line
/// produced by `split_inclusive('\n')`.
///
/// Visible at `pub(super)` so the sibling [`super::strip`] module can
/// reuse the same line-trim convention without duplicating it.
pub(super) fn strip_trailing_newline(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// `true` if `line` is exactly `` ``` `` (no info string, no other
/// chars). Used as the fence-close sentinel.
///
/// Visible at `pub(super)` so the sibling [`super::strip`] module can
/// reuse the same close-sentinel rule without duplicating it.
pub(super) fn is_bare_fence_close(line: &str) -> bool {
    line == "```"
}

/// Collapse a sequence of `split_inclusive('\n')` lines back into a
/// single string preserving newlines. Trailing newline from the last
/// line is preserved; the caller may trim if it cares.
fn collapse_free_buffer(lines: &[&str]) -> String {
    let total_len: usize = lines.iter().map(|s| s.len()).sum();
    let mut out = String::with_capacity(total_len);
    for l in lines {
        out.push_str(l);
    }
    out
}

// =========================================================================
// Internal: doc-level / per-element JSON shapes.
// =========================================================================

/// Doc-level `bgraph` block JSON — identity-only shape per v2.1.0
/// (CR-56 § I.4). `title` migrated to the `bgraph-metadata` fence.
///
/// Forward compatibility: unknown fields are silently dropped (default
/// serde behavior with `deny_unknown_fields` *not* set), so a v2.0.0
/// fixture's stray `title` field doesn't break parsing — it's just
/// ignored. The metadata fence is the canonical title source under v2.1.0.
///
/// CR-49 (v2.1.0+) added `topology` here.
/// CR-60 (2026-05-22) retracted `source_identity` + `supersedes` per the
/// byte-in/byte-out principle (arch doc 11 + DT-04). `#[serde(default)]`
/// on `topology` so absence parses to `None` — v2.1.0 graphs without it
/// stay fully accepted. Pre-CR-60 v2.1.0 inputs that still carry
/// `source_identity` or `supersedes` are silently ignored (default serde
/// behavior; `deny_unknown_fields` not set, consistent with the wider
/// forward-compat posture in this struct).
#[derive(Debug, Clone, Deserialize)]
struct DocLevelBlock {
    schema: String,
    blazegraph_version: String,
    source: DocLevelSource,
    flow_type: FlowType,
    #[serde(default)]
    topology: Option<String>,
    config_hash: String,
    graph_sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
struct DocLevelSource {
    format: String,
    filename: String,
    sha256: String,
}

/// Per-element JSON metadata as serialized by the B2 emitter.
/// Mirrors the inner shape of `DocumentNode` minus the
/// content/parent/children fields the spec excludes.
///
/// **Symmetry invariant (CR-57 AAR).** The field set here MUST stay in
/// lockstep with the emitter-side `NodeMetadata` struct in
/// `crate::graphs::serialization::markdown::node_metadata_json`. Round-trip
/// tests catch divergence; the two structs are the load-bearing pair.
#[derive(Debug, Clone, Deserialize)]
struct NodeMetadata {
    /// Used at parse time inside a `#[cfg(debug_assertions)]` block to
    /// verify parsed metadata IDs match builder-derived IDs (a tamper /
    /// version-mismatch fail-loud). Release builds compile that check
    /// out, so suppress the dead-code warning conditionally rather than
    /// dropping the field.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    id: NodeId,
    node_type: String,
    location: NodeLocation,
    text_order: u32,
    token_count: usize,
    /// CR-45: per-element verbatim Tika style projection. `#[serde(default)]`
    /// makes the field tolerant of fixtures / hand-edited inputs that
    /// omit it (forward-compat with the spec's "tolerate absent optional
    /// fields" rule). Threaded into `SemanticTreeElement.style` so the
    /// graph builder repopulates `DocumentNode.style_info`. CR-59 gated
    /// the emitter side; the parser stays tolerant of both shapes
    /// (with-style and without-style).
    #[serde(default)]
    style: Option<StyleMetadata>,
}

/// A single parsed bgraph element — the body text plus its decoded
/// metadata JSON. The body comes from outside the fence (Section /
/// Paragraph) or inside the fence above the JSON line (Header /
/// Footer / Margin).
#[derive(Debug)]
struct ParsedElement {
    body: String,
    metadata: NodeMetadata,
}

// =========================================================================
// Internal: small helpers.
// =========================================================================

fn parse_node_metadata(json_body: &str) -> Result<NodeMetadata, ParseError> {
    serde_json::from_str(json_body.trim_end_matches('\n'))
        .map_err(|source| ParseError::JsonParse { source })
}

/// Strip the leading `#`-run heading prefix from the Section body
/// line. The emitter writes `# Title\n` (preceded by a blank-line
/// separator), so when we pair body to a `bgraph-section` fence the
/// free-block buffer may carry leading blank lines and a trailing
/// newline. We:
///
/// 1. Trim outer whitespace (blank lines, trailing newline).
/// 2. Strip the leading `#`-run.
/// 3. Strip exactly one separator space.
///
/// Tolerant of any number of `#`s (the visual prefix caps at six per
/// the markdown limit, but `depth` in metadata may be higher — see
/// "Heading depth handling" in the spec).
fn strip_heading_prefix(body: &str) -> String {
    let trimmed = body.trim();
    let after_hashes = trimmed.trim_start_matches('#');
    after_hashes
        .strip_prefix(' ')
        .unwrap_or(after_hashes)
        .to_string()
}

/// Validate that the doc-level `schema` field is a major version this
/// parser has an arm for. v2.1.0+ (CR-57) accepts only `2.x` — the v1.x
/// body-inside H/F/M dispatch + v2.0.0 old variant-tag dispatch are
/// removed per the single-convention contract (CR-56 § I.5 +
/// no_fictional_users). Future major bumps add accepted prefixes here
/// in lock-step with the matching parser arms.
fn validate_schema(schema: &str) -> Result<(), ParseError> {
    if schema.starts_with("2.") {
        Ok(())
    } else {
        Err(ParseError::UnsupportedSchema(schema.to_string()))
    }
}

/// Map a wire-format `node_type` string to its `SemanticElementType`.
///
/// CR-59 removed the `"Message"` arm along with the rest of the v2.1.0
/// Message wire-format support. The enum variant survives in `types.rs`
/// as an orphan sentinel but has no parser path; inputs containing
/// `"node_type": "Message"` are rejected as `UnknownNodeType`.
fn map_node_type_to_semantic(s: &str) -> Result<SemanticElementType, ParseError> {
    match s {
        "Section" => Ok(SemanticElementType::Section),
        "Paragraph" => Ok(SemanticElementType::Paragraph),
        "Header" => Ok(SemanticElementType::Header),
        "Footer" => Ok(SemanticElementType::Footer),
        "Margin" => Ok(SemanticElementType::Margin),
        // Schema 0.7.0+ / Amendment F (B6): markdown-channel variants.
        "CodeBlock" => Ok(SemanticElementType::CodeBlock),
        "List" => Ok(SemanticElementType::List),
        "Blockquote" => Ok(SemanticElementType::Blockquote),
        "Table" => Ok(SemanticElementType::Table),
        other => Err(ParseError::UnknownNodeType(other.to_string())),
    }
}

// =========================================================================
// Tests.
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphs::serialization::markdown::emit_markdown;

    // --- shared fixture builders --------------------------------------

    /// Build a synthetic graph for round-trip tests. `nodes_in` is
    /// `(node_type, text, depth, text_order)`. The graph is built via
    /// `GraphBuilder::build_graph_deterministic` so IDs / paths /
    /// breadcrumbs match what the parser will derive.
    fn build_synthetic_graph(
        nodes_in: Vec<(&str, &str, u32, u32)>,
        title: Option<&str>,
        bookmarks: Option<BookmarkData>,
    ) -> DocumentGraph {
        let provenance = ParseProvenance {
            blazegraph_version: "0.6.0".to_string(),
            source_format: "markdown".to_string(),
            source_filename: "synthetic.md".to_string(),
            source_sha256: "synthetic-source-sha".to_string(),
            config_hash: "synthetic-config-hash".to_string(),
        };
        let id_gen = NodeIdGenerator::new(&provenance.source_sha256, &provenance.config_hash);

        let elements: Vec<SemanticTreeElement> = nodes_in
            .iter()
            .map(|(node_type, text, depth, text_order)| {
                let element_type = match *node_type {
                    "Section" => SemanticElementType::Section,
                    "Paragraph" => SemanticElementType::Paragraph,
                    "Header" => SemanticElementType::Header,
                    "Footer" => SemanticElementType::Footer,
                    "Margin" => SemanticElementType::Margin,
                    // Amendment F (B6, schema 0.7.0+).
                    "CodeBlock" => SemanticElementType::CodeBlock,
                    "List" => SemanticElementType::List,
                    "Blockquote" => SemanticElementType::Blockquote,
                    "Table" => SemanticElementType::Table,
                    other => panic!("unsupported test node type {other:?}"),
                };
                SemanticTreeElement {
                    text: text.to_string(),
                    element_type,
                    hierarchy_level: *depth,
                    text_order: *text_order,
                    physical_location: None,
                    style: None,
                    token_count: text.split_whitespace().count(),
                }
            })
            .collect();

        let mut graph = GraphBuilder::new()
            .build_graph_deterministic(elements, &id_gen, provenance)
            .expect("synthetic graph builds");
        graph.document_info.document_metadata.title = title.map(str::to_string);
        graph.document_info.bookmark_data = bookmarks;
        graph.structural_profile.flow_type = FlowType::Free;
        graph.compute_structural_profile();
        graph.compute_breadcrumbs();
        graph
    }

    fn canonical(g: &DocumentGraph) -> String {
        canonical::canonical_json(g)
    }

    // --- unit tests: doc-level / bookmarks / per-element -------------

    #[test]
    fn parse_doc_level_identity_plus_metadata_fence_round_trips_title() {
        // v2.1.0 (CR-56 § I.4): `title` lives in the bgraph-metadata
        // fence, not the doc-level block. Round-trip still recovers it.
        let graph =
            build_synthetic_graph(vec![("Paragraph", "Body.", 1, 0)], Some("Title"), None);
        let md = emit_markdown(&graph);
        let result = parse(&md, ParseOptions::default()).expect("parses");
        let info = &result.graph.document_info;
        assert_eq!(info.document_metadata.title.as_deref(), Some("Title"));
        let prov = info.parse_provenance.as_ref().expect("provenance");
        assert_eq!(prov.blazegraph_version, "0.6.0");
        assert_eq!(prov.source_format, "markdown");
        assert_eq!(prov.source_filename, "synthetic.md");
        assert_eq!(prov.source_sha256, "synthetic-source-sha");
        assert_eq!(prov.config_hash, "synthetic-config-hash");
        assert!(matches!(
            result.graph.structural_profile.flow_type,
            FlowType::Free
        ));
    }

    #[test]
    fn parse_bookmarks_fence_when_present() {
        let bookmarks = BookmarkData {
            sections: vec![
                BookmarkSection {
                    title: "Intro".to_string(),
                    order: 0,
                    level: 1,
                },
                BookmarkSection {
                    title: "Background".to_string(),
                    order: 1,
                    level: 2,
                },
            ],
        };
        let graph = build_synthetic_graph(
            vec![("Section", "Intro", 1, 0)],
            Some("Doc"),
            Some(bookmarks.clone()),
        );
        let md = emit_markdown(&graph);
        let result = parse(&md, ParseOptions::default()).expect("parses with bookmarks");
        let parsed_bm = result
            .graph
            .document_info
            .bookmark_data
            .expect("bookmark_data is Some");
        assert_eq!(parsed_bm.sections.len(), 2);
        assert_eq!(parsed_bm.sections[0].title, "Intro");
        assert_eq!(parsed_bm.sections[0].level, 1);
        assert_eq!(parsed_bm.sections[1].title, "Background");
        assert_eq!(parsed_bm.sections[1].level, 2);
    }

    #[test]
    fn parse_bookmarks_fence_absent_yields_none_bookmark_data() {
        let graph = build_synthetic_graph(vec![("Paragraph", "Body.", 1, 0)], None, None);
        let md = emit_markdown(&graph);
        let result = parse(&md, ParseOptions::default()).expect("parses");
        assert!(result.graph.document_info.bookmark_data.is_none());
    }

    #[test]
    fn parse_section_pairs_with_preceding_heading() {
        let graph = build_synthetic_graph(vec![("Section", "Introduction", 1, 0)], None, None);
        let md = emit_markdown(&graph);
        let result = parse(&md, ParseOptions::default()).expect("parses");
        // The Section node should carry "Introduction" as its content.
        let section = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "Section")
            .expect("section present");
        assert_eq!(section.content.text, "Introduction");
    }

    #[test]
    fn parse_paragraph_pairs_with_preceding_text() {
        let graph = build_synthetic_graph(vec![("Paragraph", "Hello world.", 1, 0)], None, None);
        let md = emit_markdown(&graph);
        let result = parse(&md, ParseOptions::default()).expect("parses");
        let para = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "Paragraph")
            .expect("paragraph present");
        assert_eq!(para.content.text, "Hello world.");
    }

    #[test]
    fn parse_header_extracts_outside_fence_body_v2() {
        // v2.0.0 path: H/F/M body lives on the line preceding the
        // fence, same shape as Paragraph.
        let graph =
            build_synthetic_graph(vec![("Header", "Running header text", 1, 0)], None, None);
        let md = emit_markdown(&graph);
        let result = parse(&md, ParseOptions::default()).expect("parses");
        let header = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "Header")
            .expect("header present");
        assert_eq!(header.content.text, "Running header text");
    }

    #[test]
    fn parse_rejects_v1_x_schemas() {
        // v2.1.0+ (CR-57): single convention. v1.x fixtures (body-inside
        // H/F/M, no bgraph-metadata fence) are no longer parseable. No
        // back-compat dispatch — fixtures regenerate via the emit path.
        let v1_fixture =
            "```bgraph\n\
             {\"schema\":\"1.1.0\",\"blazegraph_version\":\"0.6.0\",\"source\":{\"format\":\"markdown\",\"filename\":\"x.md\",\"sha256\":\"a\"},\"flow_type\":\"Free\",\"title\":null,\"config_hash\":\"b\",\"graph_sha256\":\"c\"}\n\
             ```\n";
        let result = parse(v1_fixture, ParseOptions { accept_drift: true });
        assert!(
            matches!(result, Err(ParseError::UnsupportedSchema(_))),
            "v1.x schemas must be rejected under v2.1.0+ single-convention parser; got: {result:?}"
        );
    }

    #[test]
    fn parse_rejects_old_kebab_case_variant_tags() {
        // F-11 + single-convention: bgraph-codeblock / bgraph-blockquote
        // (v2.0.0 tag names) must be rejected. The walk picks them up as
        // unrecognized tags via the fence-tag dispatch.
        let bad_codeblock = format!(
            "```bgraph\n\
             {{\"schema\":\"2.0.0\",\"blazegraph_version\":\"0.6.0\",\"source\":{{\"format\":\"markdown\",\"filename\":\"x.md\",\"sha256\":\"a\"}},\"flow_type\":\"Free\",\"config_hash\":\"b\",\"graph_sha256\":\"c\"}}\n\
             ```\n\
             \n\
             ```bgraph-metadata\n{{}}\n```\n\
             \n\
             body\n\
             ```bgraph-codeblock\n{{\"id\":\"00000000-0000-0000-0000-000000000000\",\"node_type\":\"CodeBlock\",\"location\":{{\"semantic\":{{\"path\":\"1\",\"depth\":1,\"breadcrumbs\":[]}},\"physical\":null}},\"text_order\":0,\"token_count\":1}}\n```\n",
        );
        let result = parse(&bad_codeblock, ParseOptions { accept_drift: true });
        assert!(
            matches!(result, Err(ParseError::MalformedFence(_))),
            "old `bgraph-codeblock` tag must be rejected under v2.1.0+; got: {result:?}"
        );
    }

    #[test]
    fn parse_missing_doc_level_block_errors() {
        // Hand-craft a stream with no doc-level fence.
        let bogus = "# Title\n\nSome prose.\n";
        // The sniff in `parse_markdown` returns
        // `GenericMarkdownNotYetSupported`; calling `parse` directly
        // surfaces `MissingDocLevelBlock`.
        let result = parse(bogus, ParseOptions::default());
        assert!(matches!(result, Err(ParseError::MissingDocLevelBlock)));
    }

    #[test]
    fn parse_unknown_node_type_errors() {
        // Build a real bgraph.md, then surgically replace one
        // node_type with garbage so we hit UnknownNodeType.
        let graph = build_synthetic_graph(vec![("Paragraph", "Body.", 1, 0)], None, None);
        let md = emit_markdown(&graph);
        let tampered = md.replace("\"node_type\":\"Paragraph\"", "\"node_type\":\"Gibberish\"");
        let result = parse(&tampered, ParseOptions { accept_drift: true });
        assert!(matches!(result, Err(ParseError::UnknownNodeType(_))));
    }

    #[test]
    fn parse_token_count_is_read_from_metadata() {
        // The emitter writes `token_count` from the source node; the
        // parser must read it through to the reconstructed graph.
        let graph = build_synthetic_graph(
            vec![("Paragraph", "one two three four five", 1, 0)],
            None,
            None,
        );
        let md = emit_markdown(&graph);
        let result = parse(&md, ParseOptions::default()).expect("parses");
        let para = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "Paragraph")
            .expect("paragraph");
        assert_eq!(para.token_count, 5);
    }

    #[test]
    fn parse_strict_mode_rejects_drift() {
        let graph = build_synthetic_graph(vec![("Paragraph", "Original.", 1, 0)], None, None);
        let md = emit_markdown(&graph);
        // Mutate the body so graph_sha256 changes.
        let tampered = md.replace("Original.", "Tampered.");
        let result = parse(&tampered, ParseOptions::default());
        assert!(matches!(result, Err(ParseError::HashMismatch { .. })));
    }

    #[test]
    fn parse_accept_drift_mode_returns_derivative() {
        let graph = build_synthetic_graph(vec![("Paragraph", "Original.", 1, 0)], None, None);
        let md = emit_markdown(&graph);
        let tampered = md.replace("Original.", "Tampered.");
        let result =
            parse(&tampered, ParseOptions { accept_drift: true }).expect("parses with drift");
        match result.identity {
            ParseIdentity::Derivative {
                original_sha256,
                recomputed_sha256,
            } => {
                assert_ne!(original_sha256, recomputed_sha256);
            }
            other => panic!("expected Derivative, got {other:?}"),
        }
    }

    #[test]
    fn parse_round_trip_canonical_bytes_match() {
        // The smallest non-trivial test: emit a Section+Paragraph
        // graph, parse it back, assert canonical(parsed) ==
        // canonical(original).
        let original = build_synthetic_graph(
            vec![
                ("Section", "Intro", 1, 0),
                ("Paragraph", "Hello world.", 1, 1),
            ],
            Some("Doc"),
            None,
        );
        let md = emit_markdown(&original);
        let result = parse(&md, ParseOptions::default()).expect("parses cleanly");
        assert!(matches!(result.identity, ParseIdentity::Verified));
        assert_eq!(canonical(&result.graph), canonical(&original));
    }

    #[test]
    fn parse_reserved_prefix_in_body_errors() {
        // Hand-craft a malformed bgraph.md where a body line opens a
        // new bgraph fence inside an active fence. (The scanner sees
        // this as a reserved-prefix violation.)
        let bogus = "```bgraph\n\
                     {\"schema\":\"2.1.0\",\"blazegraph_version\":\"0.6.0\",\"source\":{\"format\":\"markdown\",\"filename\":\"x.md\",\"sha256\":\"a\"},\"flow_type\":\"Free\",\"config_hash\":\"b\",\"graph_sha256\":\"c\"}\n\
                     ```bgraph-section\n\
                     ```\n";
        // The inner `bgraph-section` open without a preceding ``` close is
        // ambiguous to the scanner — it picks up as a body-line `bgraph`
        // prefix inside the doc-level fence, triggering ReservedPrefixInBody.
        let result = parse(bogus, ParseOptions::default());
        assert!(
            matches!(result, Err(ParseError::ReservedPrefixInBody))
                || matches!(result, Err(ParseError::MalformedFence(_))),
            "got: {result:?}",
        );
    }

    /// Diagnostic — always passes. Run with `--nocapture` to see a
    /// sample emit → parse → canonical-equal round-trip.
    #[test]
    fn parse_diagnostic_print_sample_output() {
        let bookmarks = BookmarkData {
            sections: vec![BookmarkSection {
                title: "Introduction".to_string(),
                order: 0,
                level: 1,
            }],
        };
        let original = build_synthetic_graph(
            vec![
                ("Section", "Introduction", 1, 0),
                ("Paragraph", "First paragraph body.", 1, 1),
                ("Header", "Running header", 1, 2),
                ("Footer", "Confidential", 1, 3),
            ],
            Some("Sample Document"),
            Some(bookmarks),
        );
        let md = emit_markdown(&original);
        eprintln!("--- BEGIN bgraph.md sample ---");
        eprintln!("{md}");
        eprintln!("--- END bgraph.md sample ---");

        let result = parse(&md, ParseOptions::default()).expect("parses cleanly");
        eprintln!("--- ParseIdentity: {:?} ---", result.identity);
        eprintln!(
            "--- Canonical-bytes match: {} ---",
            canonical(&result.graph) == canonical(&original)
        );
        assert_eq!(canonical(&result.graph), canonical(&original));
    }

    // --- scanner unit tests ------------------------------------------

    #[test]
    fn strip_heading_prefix_removes_hashes_and_one_space() {
        assert_eq!(strip_heading_prefix("# Hello"), "Hello");
        assert_eq!(strip_heading_prefix("## Hello"), "Hello");
        assert_eq!(strip_heading_prefix("###### Deep"), "Deep");
        // Multiple spaces after `#`: only one is consumed.
        assert_eq!(strip_heading_prefix("#  Two"), " Two");
    }

    #[test]
    fn validate_schema_accepts_only_v2_x() {
        // v2.1.0+ (CR-57): single convention. v1.x and v3.x+ are rejected.
        for ok in ["2.0.0", "2.1.0", "2.42.7", "2.99.0"] {
            assert!(
                validate_schema(ok).is_ok(),
                "{ok} should be accepted under v2.1.0+ parser"
            );
        }
    }

    #[test]
    fn validate_schema_rejects_v1_and_v3_plus() {
        for bad in [
            "0.9.0", "1.0.0", "1.1.0", "1.42.0", "3.0.0", "4.0.0", "10.0.0",
        ] {
            assert!(
                matches!(validate_schema(bad), Err(ParseError::UnsupportedSchema(_))),
                "{bad} should be rejected by v2.1.0+ single-convention parser"
            );
        }
    }

    // ----- Amendment F (B6, schema 0.7.0+) parse tests ---------------

    #[test]
    fn parse_codeblock_fence_recovers_inner_fenced_block_as_body() {
        let raw_codeblock = "```rust\nfn main() {}\n```";
        let original =
            build_synthetic_graph(vec![("CodeBlock", raw_codeblock, 1, 0)], Some("Doc"), None);
        let md = emit_markdown(&original);
        let result = parse(&md, ParseOptions::default()).expect("parses cleanly");
        assert!(matches!(result.identity, ParseIdentity::Verified));
        // The reconstructed CodeBlock's body should equal the
        // verbatim raw fence + body + closing fence.
        let codeblock = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "CodeBlock")
            .expect("CodeBlock present");
        assert_eq!(codeblock.content.text, raw_codeblock);
    }

    #[test]
    fn parse_list_fence_recovers_multi_item_body() {
        let raw_list = "- one\n- two\n- three";
        let original = build_synthetic_graph(vec![("List", raw_list, 1, 0)], Some("Doc"), None);
        let md = emit_markdown(&original);
        let result = parse(&md, ParseOptions::default()).expect("parses cleanly");
        assert!(matches!(result.identity, ParseIdentity::Verified));
        let list = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "List")
            .expect("List present");
        assert_eq!(list.content.text, raw_list);
    }

    #[test]
    fn parse_list_fence_handles_nested_indented_continuation() {
        let raw_list = "- top\n  - nested\n  - also nested\n- top two";
        let original = build_synthetic_graph(vec![("List", raw_list, 1, 0)], Some("Doc"), None);
        let md = emit_markdown(&original);
        let result = parse(&md, ParseOptions::default()).expect("parses cleanly");
        let list = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "List")
            .expect("List present");
        assert_eq!(list.content.text, raw_list);
    }

    #[test]
    fn parse_blockquote_fence_recovers_gt_prefixed_body() {
        let raw_quote = "> a quote\n> still quoted";
        let original =
            build_synthetic_graph(vec![("Blockquote", raw_quote, 1, 0)], Some("Doc"), None);
        let md = emit_markdown(&original);
        let result = parse(&md, ParseOptions::default()).expect("parses cleanly");
        let bq = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "Blockquote")
            .expect("Blockquote present");
        assert_eq!(bq.content.text, raw_quote);
    }

    #[test]
    fn parse_table_fence_recovers_gfm_body_with_alignment_row() {
        let raw_table = "| h1 | h2 |\n|---|---|\n| a | b |";
        let original = build_synthetic_graph(vec![("Table", raw_table, 1, 0)], Some("Doc"), None);
        let md = emit_markdown(&original);
        let result = parse(&md, ParseOptions::default()).expect("parses cleanly");
        let tbl = result
            .graph
            .nodes
            .values()
            .find(|n| n.node_type == "Table")
            .expect("Table present");
        assert_eq!(tbl.content.text, raw_table);
    }

    #[test]
    fn parse_round_trip_with_reserved_prefix_in_body_self_referential() {
        // v2.0.0 escape contract: a Paragraph body containing the
        // literal reserved prefix `` ```bgraph-section `` must
        // round-trip cleanly through emit → parse → canonical-equal.
        // This is the "blazegraph parses its own docs" acceptance test.
        let prose_with_fence = "Example bgraph fence:\n\
                                ```bgraph-section\n\
                                {\"id\":\"...\",\"node_type\":\"Section\"}\n\
                                ```\n\
                                End of example.";
        let original = build_synthetic_graph(
            vec![("Paragraph", prose_with_fence, 1, 0)],
            Some("Self-Referential"),
            None,
        );
        // NodeContent::new should have escaped the reserved-prefix
        // line — assert that before continuing.
        let para = original
            .nodes
            .values()
            .find(|n| n.node_type == "Paragraph")
            .expect("paragraph present");
        assert!(
            para.content.text.contains("\\```bgraph-section"),
            "NodeContent::new must escape ```bgraph-section to \\```bgraph-section; \
             got: {:?}",
            para.content.text
        );

        // Round-trip: the escaped body lets the scanner see only the
        // real bgraph-paragraph fence as a fence open.
        let md = emit_markdown(&original);
        let result = parse(&md, ParseOptions::default())
            .expect("self-referential paragraph round-trips cleanly");
        assert!(matches!(result.identity, ParseIdentity::Verified));
        assert_eq!(canonical(&result.graph), canonical(&original));
    }

    // ----- CR-49 (v2.1.0+) parse tests --------------------------------
    // CR-60 (2026-05-22) retracted source_identity + supersedes per
    // arch doc 11 + DT-04 (byte-in/byte-out). Only `topology` remains
    // from the CR-49 doc-level additions; this test covers its round-trip.

    #[test]
    fn parse_doc_level_topology_round_trip() {
        // Round-trip a doc-level block carrying `topology`.
        // The field survives emit → parse → re-emit byte-for-byte.
        let mut original = build_synthetic_graph(
            vec![("Paragraph", "Body.", 1, 0)],
            Some("Topology Test"),
            None,
        );
        original.document_info.topology = Some("stream".to_string());
        let md1 = emit_markdown(&original);
        let result = parse(&md1, ParseOptions::default()).expect("round-trips");
        assert!(matches!(result.identity, ParseIdentity::Verified));
        let info = &result.graph.document_info;
        assert_eq!(info.topology.as_deref(), Some("stream"));
        // Second emit must be byte-identical to the first.
        let md2 = emit_markdown(&result.graph);
        assert_eq!(md1, md2, "second emit must be byte-identical to the first");
    }

    // CR-59 removed `parse_message_variant_round_trip_byte_identity` and
    // `parse_bgraph_message_variant_tag` along with the wire-format
    // support for the Message variant. See `types.rs` for the orphan
    // sentinel that survives.

    #[test]
    fn parse_rejects_bgraph_message_fence_post_cr59() {
        // CR-59 retracted `bgraph-message`. A synthetic v2.1.0 fixture
        // carrying the tag must be rejected at parse time. The fence-tag
        // recognizer now classifies `bgraph-message` as a reserved-prefix
        // violation (returns Some("bgraph-message") so the caller sees it
        // as a fence, but the dispatch arm in `parse` no longer accepts
        // it and falls through to MalformedFence).
        let base = build_synthetic_graph(vec![("Paragraph", "Body.", 1, 0)], None, None);
        let md = emit_markdown(&base);
        // Splice a `bgraph-message` fence into a synthetic shape. Use a
        // hand-built fence with the minimum JSON the parser would have
        // tried to dispatch on.
        let injected = format!(
            "{md}Hello.\n```bgraph-message\n{{\"id\":\"00000000-0000-0000-0000-000000000000\",\"node_type\":\"Message\",\"location\":{{\"semantic\":{{\"path\":\"2\",\"depth\":1,\"breadcrumbs\":[]}},\"physical\":null}},\"text_order\":1,\"token_count\":1}}\n```\n"
        );
        let result = parse(&injected, ParseOptions::default());
        assert!(
            matches!(result, Err(ParseError::MalformedFence(_))),
            "bgraph-message must be rejected post-CR-59; got {result:?}"
        );
    }

    #[test]
    fn parse_doc_level_topology_absent_yields_none() {
        // A bgraph.md without `topology` parses cleanly; the
        // reconstructed graph carries `None`.
        let graph = build_synthetic_graph(vec![("Paragraph", "Body.", 1, 0)], None, None);
        let md = emit_markdown(&graph);
        let result = parse(&md, ParseOptions::default()).expect("parses");
        let info = &result.graph.document_info;
        assert!(info.topology.is_none());
    }

    #[test]
    fn parse_mixed_amendment_f_variants_round_trip() {
        // One of each new variant in the same doc — verify the
        // ordering and pairing work when multiple body-outside
        // fences interleave.
        let original = build_synthetic_graph(
            vec![
                ("Section", "Intro", 1, 0),
                ("Paragraph", "Some prose.", 1, 1),
                ("CodeBlock", "```rust\nfn x() {}\n```", 1, 2),
                ("List", "- a\n- b", 1, 3),
                ("Blockquote", "> q", 1, 4),
                ("Table", "| h |\n|---|\n| c |", 1, 5),
            ],
            Some("Mixed"),
            None,
        );
        let md = emit_markdown(&original);
        let result = parse(&md, ParseOptions::default()).expect("parses cleanly");
        assert!(matches!(result.identity, ParseIdentity::Verified));
        assert_eq!(canonical(&result.graph), canonical(&original));
    }
}
