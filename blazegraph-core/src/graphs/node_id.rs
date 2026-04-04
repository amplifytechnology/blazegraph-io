//! Deterministic Node ID Generation
//!
//! Generates reproducible UUIDv5 node IDs from a document-scoped namespace.
//! The contract: same blazegraph version + same PDF + same config = same node IDs.
//!
//! The namespace is derived from:
//!   UUIDv5(BLAZEGRAPH_NS, "{blazegraph_version}:{pdf_hash}:{config_hash}")
//!
//! Each node ID is then:
//!   UUIDv5(document_namespace, "root")     — for the document root
//!   UUIDv5(document_namespace, "0")        — for text_order 0
//!   UUIDv5(document_namespace, "1")        — for text_order 1
//!   ...
//!
//! This means node IDs are:
//! - Deterministic: identical inputs produce identical IDs
//! - Globally unique: different documents/configs/versions produce different IDs
//! - Standard UUIDs: compatible with any system expecting UUID format

use crate::types::NodeId;
use uuid::Uuid;

/// Fixed namespace UUID for all Blazegraph node IDs.
/// Computed as UUIDv5(DNS, "blazegraph.io") = a6f4212f-b2b3-5e5f-a124-e4f54c8bc5f9
const BLAZEGRAPH_NS: Uuid = Uuid::from_bytes([
    0xa6, 0xf4, 0x21, 0x2f, 0xb2, 0xb3, 0x5e, 0x5f,
    0xa1, 0x24, 0xe4, 0xf5, 0x4c, 0x8b, 0xc5, 0xf9,
]);

/// Generates deterministic node IDs scoped to a specific document parse.
#[derive(Debug, Clone)]
pub struct NodeIdGenerator {
    /// Document-scoped namespace: UUIDv5(BLAZEGRAPH_NS, "{version}:{pdf_hash}:{config_hash}")
    document_namespace: Uuid,
}

impl NodeIdGenerator {
    /// Create a new generator for a specific document parse.
    ///
    /// # Arguments
    /// * `blazegraph_version` - Parser version (e.g., "0.1.1")
    /// * `pdf_hash` - Hash of the PDF content
    /// * `config_hash` - Hash of the parsing config
    pub fn new(blazegraph_version: &str, pdf_hash: &str, config_hash: &str) -> Self {
        let scope = format!("{}:{}:{}", blazegraph_version, pdf_hash, config_hash);
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
        let gen1 = NodeIdGenerator::new("0.1.1", "abc123", "def456");
        let gen2 = NodeIdGenerator::new("0.1.1", "abc123", "def456");

        assert_eq!(gen1.root_id(), gen2.root_id());
        assert_eq!(gen1.node_id(0), gen2.node_id(0));
        assert_eq!(gen1.node_id(42), gen2.node_id(42));
    }

    #[test]
    fn test_different_inputs_different_ids() {
        let gen_a = NodeIdGenerator::new("0.1.1", "abc123", "def456");
        let gen_b = NodeIdGenerator::new("0.1.1", "abc123", "different_config");
        let gen_c = NodeIdGenerator::new("0.2.0", "abc123", "def456");

        // Different config → different IDs
        assert_ne!(gen_a.root_id(), gen_b.root_id());
        assert_ne!(gen_a.node_id(0), gen_b.node_id(0));

        // Different version → different IDs
        assert_ne!(gen_a.root_id(), gen_c.root_id());
    }

    #[test]
    fn test_root_differs_from_nodes() {
        let gen = NodeIdGenerator::new("0.1.1", "abc123", "def456");

        assert_ne!(gen.root_id(), gen.node_id(0));
        assert_ne!(gen.root_id(), gen.node_id(1));
    }

    #[test]
    fn test_node_ids_are_unique() {
        let gen = NodeIdGenerator::new("0.1.1", "abc123", "def456");

        let ids: Vec<NodeId> = (0..100).map(|i| gen.node_id(i)).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len());
    }

    #[test]
    fn test_ids_are_valid_uuids() {
        let gen = NodeIdGenerator::new("0.1.1", "abc123", "def456");

        let root = gen.root_id();
        let node = gen.node_id(42);

        // UUIDv5 has version nibble = 5
        assert_eq!(root.get_version_num(), 5);
        assert_eq!(node.get_version_num(), 5);
    }
}
