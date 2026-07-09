//! Canonical JSON serialization for graph hashing and round-trip identity.
//!
//! "Canonical" means: deterministic byte output for any given
//! `DocumentGraph`. Same input → same bytes across runs, platforms, and
//! `serde_json` patch versions (within reason — we rely on
//! `serde_json`'s default float formatting, which is the shortest-
//! round-trip decimal representation; that is RFC 8785 / JCS aligned by
//! behavior, not by transitive crate dep).
//!
//! Used as the input to `graph_sha256`, which the bgraph.md forward
//! emitter (B2) embeds in the document-level block, and which the
//! reverse parser (B3) recomputes for identity verification.
//!
//! ## Canonical-input invariant
//!
//! `canonical_json` operates on `DocumentGraph` (the in-memory graph
//! type), which by design contains no time-, environment-, or
//! otherwise non-deterministic fields. File-emission metadata — the
//! `created_at` timestamp and the `schema_version` string — lives on
//! `SortedDocumentGraph` (the on-disk wrapper), not on `DocumentGraph`.
//! Any future schema addition that introduces run-time-variable state
//! MUST land on the wrapper; the canonical-input invariant is a
//! structural property, not a maintained allowlist.
//!
//! See `docs/P2/core/architecture/08-bgraph-md-format.md` for the full
//! contract.

use crate::types::DocumentGraph;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Serialize a graph to canonical JSON: sorted object keys,
/// default-compact, shortest-round-trip floats. Used as the input to
/// [`graph_sha256`].
///
/// `serde_json`'s default `to_string` is already compact and emits
/// shortest-round-trip floats — the only piece this function adds is
/// sorted-key output. Implementation: serialize the graph to a
/// `serde_json::Value`, then walk the `Value` tree producing JSON with
/// object keys sorted lexicographically.
///
/// Content-body invariant (Block A / Amendment M, arch-14 §3.1):
/// `DocumentGraph` *is* the content body — provenance, derived
/// aggregates (`structural_profile`), and envelope fields
/// (`graph_sha256` itself, `schema_version`, `created_at`) live on the
/// `SortedDocumentGraph` wrapper or are threaded as explicit values,
/// never on this type. The body is hashed whole: canonical_json has no
/// per-field exclusion list, and must never grow one — a field stored
/// in the body but excluded from the hash would break `bytes ==
/// identity` for every content-addressed consumer downstream (the
/// CAS/URD composability argument, arch-11).
pub fn canonical_json(graph: &DocumentGraph) -> String {
    let value = serde_json::to_value(graph).expect("DocumentGraph is always JSON-serializable");
    let mut out = String::new();
    write_canonical(&value, &mut out);
    out
}

/// SHA-256 of `canonical_json(graph)`, hex-encoded (lowercase). Stable
/// across runs of the same logical graph.
pub fn graph_sha256(graph: &DocumentGraph) -> String {
    let canonical = canonical_json(graph);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Walk a `serde_json::Value` and write it to `out` with object keys
/// sorted lexicographically. Numbers, strings, booleans, and null are
/// delegated to `serde_json::to_string` (compact, shortest-round-trip
/// floats); arrays preserve their order; objects emit keys in sorted
/// order.
fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            // Leaf scalars: serde_json's default rendering is exactly
            // what we want (compact, shortest-round-trip floats).
            out.push_str(&serde_json::to_string(value).expect("leaf scalar is serializable"));
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            // Sort keys lexicographically. `serde_json::Map` preserves
            // insertion order by default; pulling keys out and sorting
            // is the cheapest path to canonical output.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                // Re-encode the key as a JSON string so escapes are
                // applied (serde_json gives us this for free).
                out.push_str(
                    &serde_json::to_string(*key).expect("object key is always serializable"),
                );
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    /// Build a minimal `DocumentGraph` by hand (no `GraphBuilder`).
    /// Independent of the builder's signature so this test module
    /// compiles regardless of where in the commit sequence it lands.
    fn build_minimal_graph(seed: &str) -> DocumentGraph {
        let root_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, format!("{seed}:root").as_bytes());
        let para_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, format!("{seed}:0").as_bytes());

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
                internal_refs: vec![],
                external_refs: vec![],
                confidence: 0,
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
                    text: "Hello.".to_string(),
                },
                style_info: None,
                token_count: 2,
                parent: Some(root_id),
                children: Vec::new(),
                internal_refs: vec![],
                external_refs: vec![],
                confidence: 0,
            },
        );

        DocumentGraph {
            nodes,
            document_info: DocumentInfo {
                root_id,
                kind: crate::types::default_kind(),
                document_metadata: DocumentMetadata::default(),
                outline_data: None,
                flow_type: FlowType::default(),
                topology: None,
            },
        }
    }

    #[test]
    fn canonical_json_is_deterministic_across_runs() {
        // Codifies the canonical-input invariant from
        // docs/P2/core/architecture/08-bgraph-md-format.md: same logical
        // graph → byte-identical canonical output, regardless of
        // wall-clock time between builds. If this fails, a non-
        // deterministic field has snuck onto `DocumentGraph` — it must
        // be moved to `SortedDocumentGraph` (the on-disk wrapper).
        let g1 = build_minimal_graph("seed");
        // Non-trivial delay between builds — would surface a
        // `Utc::now()` snuck into the graph.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let g2 = build_minimal_graph("seed");
        assert_eq!(canonical_json(&g1), canonical_json(&g2));
    }

    #[test]
    fn graph_sha256_length_is_64_hex() {
        let graph = build_minimal_graph("seed");
        let h = graph_sha256(&graph);
        assert_eq!(h.len(), 64, "expected 64 hex chars, got {h:?}");
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "expected lowercase hex chars, got {h:?}",
        );
    }

    #[test]
    fn graph_sha256_is_stable_across_runs() {
        let g1 = build_minimal_graph("seed");
        let g2 = build_minimal_graph("seed");
        assert_eq!(graph_sha256(&g1), graph_sha256(&g2));
    }

    #[test]
    fn graph_sha256_differs_for_different_graphs() {
        let g_a = build_minimal_graph("seed-a");
        let g_b = build_minimal_graph("seed-b");
        assert_ne!(graph_sha256(&g_a), graph_sha256(&g_b));
    }

    #[test]
    fn canonical_json_keys_are_sorted() {
        // Spot-check: in canonical output the document_info object's
        // first key must be the lex-smallest of its present keys.
        // Without outline_data (skip_serializing_if = None), the
        // present keys are: document_metadata, kind, root_id
        // → smallest is "document_metadata".
        let graph = build_minimal_graph("seed");
        let canonical = canonical_json(&graph);
        let prefix = r#""document_info":{"document_metadata":"#;
        assert!(
            canonical.contains(prefix),
            "canonical output should have sorted document_info keys; got {}",
            &canonical[..canonical.len().min(500)],
        );
    }
}
