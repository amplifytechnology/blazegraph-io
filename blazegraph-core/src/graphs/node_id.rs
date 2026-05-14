//! Deterministic Node ID Generation
//!
//! Generates reproducible UUIDv5 node IDs from a document-scoped namespace.
//! The contract: same source bytes + same parser config = same node IDs,
//! regardless of which blazegraph version performed the parse.
//!
//! The namespace is derived from:
//!   UUIDv5(BLAZEGRAPH_NS, "{source_hash}:{config_hash}")
//!
//! Each node ID is then:
//!   UUIDv5(document_namespace, "root")     — for the document root
//!   UUIDv5(document_namespace, "0")        — for text_order 0
//!   UUIDv5(document_namespace, "1")        — for text_order 1
//!   ...
//!
//! This means node IDs are:
//! - Deterministic: identical inputs produce identical IDs.
//! - Globally unique: different documents/configs produce different IDs.
//! - Version-invariant: a parser version bump does not invalidate IDs
//!   for the same `(source, config)`. External systems that reference
//!   node IDs (URD chunk addresses, attestation chains, future link
//!   nodes between graphs) survive blazegraph upgrades. See CR-47.
//! - Standard UUIDs: compatible with any system expecting UUID format.

use crate::types::NodeId;
use uuid::Uuid;

/// Fixed namespace UUID for all Blazegraph node IDs.
/// Computed as UUIDv5(DNS, "blazegraph.io") = a6f4212f-b2b3-5e5f-a124-e4f54c8bc5f9
const BLAZEGRAPH_NS: Uuid = Uuid::from_bytes([
    0xa6, 0xf4, 0x21, 0x2f, 0xb2, 0xb3, 0x5e, 0x5f, 0xa1, 0x24, 0xe4, 0xf5, 0x4c, 0x8b, 0xc5, 0xf9,
]);

/// Generates deterministic node IDs scoped to a specific document parse.
#[derive(Debug, Clone)]
pub struct NodeIdGenerator {
    /// Document-scoped namespace: UUIDv5(BLAZEGRAPH_NS, "{source_hash}:{config_hash}")
    document_namespace: Uuid,
}

impl NodeIdGenerator {
    /// Create a new generator for a specific document parse.
    ///
    /// CR-47: `blazegraph_version` is intentionally *not* part of the
    /// namespace input. Node IDs identify positional slots within a
    /// parse of `(source, config)`; parser version bumps that change
    /// graph shape are surfaced by `graph_sha256`, not by changing
    /// every node ID. This lets external systems (URD chunks, link
    /// nodes, attestation chains) reference node IDs stably across
    /// blazegraph releases.
    ///
    /// # Arguments
    /// * `source_hash` - Full SHA-256 of the source-format content
    ///   (PDF bytes for the PDF channel; markdown bytes for MD; DOCX
    ///   bytes for DOCX). Produced by [`crate::storage::calculate_source_hash`].
    /// * `config_hash` - Hash of the parsing config.
    pub fn new(source_hash: &str, config_hash: &str) -> Self {
        let scope = format!("{}:{}", source_hash, config_hash);
        let document_namespace = Uuid::new_v5(&BLAZEGRAPH_NS, scope.as_bytes());
        Self { document_namespace }
    }

    /// Generate the root node ID.
    pub fn root_id(&self) -> NodeId {
        Uuid::new_v5(&self.document_namespace, b"root")
    }

    /// Generate a node ID for a given text order position.
    pub fn node_id(&self, text_order: u32) -> NodeId {
        Uuid::new_v5(&self.document_namespace, text_order.to_string().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_ids() {
        let gen1 = NodeIdGenerator::new("abc123", "def456");
        let gen2 = NodeIdGenerator::new("abc123", "def456");

        assert_eq!(gen1.root_id(), gen2.root_id());
        assert_eq!(gen1.node_id(0), gen2.node_id(0));
        assert_eq!(gen1.node_id(42), gen2.node_id(42));
    }

    #[test]
    fn test_different_inputs_different_ids() {
        let gen_a = NodeIdGenerator::new("abc123", "def456");
        let gen_b = NodeIdGenerator::new("abc123", "different_config");
        let gen_c = NodeIdGenerator::new("different_source", "def456");

        // Different config → different IDs.
        assert_ne!(gen_a.root_id(), gen_b.root_id());
        assert_ne!(gen_a.node_id(0), gen_b.node_id(0));

        // Different source → different IDs.
        assert_ne!(gen_a.root_id(), gen_c.root_id());
        assert_ne!(gen_a.node_id(0), gen_c.node_id(0));
    }

    /// CR-47 core invariant: `NodeIdGenerator::new` takes only
    /// `(source_hash, config_hash)`. There is no parser-version
    /// parameter to vary, so the IDs a caller gets back are
    /// version-invariant by construction. This test pins that:
    /// the same `(source, config)` produced under "different versions"
    /// (which would have used different namespace strings under the
    /// pre-CR-47 signature) now produces identical IDs.
    #[test]
    fn test_node_ids_are_version_invariant() {
        // Under the old `(version, source, config)` signature, these
        // two contexts produced different IDs. Under CR-47 they MUST
        // produce the same IDs because the signature dropped version.
        let from_old_version = NodeIdGenerator::new("abc123", "def456");
        let from_new_version = NodeIdGenerator::new("abc123", "def456");

        assert_eq!(from_old_version.root_id(), from_new_version.root_id());
        for text_order in [0u32, 1, 7, 42, 1024] {
            assert_eq!(
                from_old_version.node_id(text_order),
                from_new_version.node_id(text_order),
                "node_id({text_order}) must match across parser versions"
            );
        }
    }

    #[test]
    fn test_root_differs_from_nodes() {
        let gen = NodeIdGenerator::new("abc123", "def456");

        assert_ne!(gen.root_id(), gen.node_id(0));
        assert_ne!(gen.root_id(), gen.node_id(1));
    }

    #[test]
    fn test_node_ids_are_unique() {
        let gen = NodeIdGenerator::new("abc123", "def456");

        let ids: Vec<NodeId> = (0..100).map(|i| gen.node_id(i)).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len());
    }

    #[test]
    fn test_ids_are_valid_uuids() {
        let gen = NodeIdGenerator::new("abc123", "def456");

        let root = gen.root_id();
        let node = gen.node_id(42);

        // UUIDv5 has version nibble = 5.
        assert_eq!(root.get_version_num(), 5);
        assert_eq!(node.get_version_num(), 5);
    }
}
