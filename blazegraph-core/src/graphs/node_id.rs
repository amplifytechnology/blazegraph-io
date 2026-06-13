//! Content-Derived Node ID Generation (CR-83)
//!
//! Generates reproducible UUIDv5 node IDs from a node's own **content**
//! plus its **breadcrumb path** (ancestor-heading trail) plus an
//! **occurrence** index. There is no per-document namespace.
//!
//!   node_id = UUIDv5(BLAZEGRAPH_NS,
//!                    breadcrumb_path ‖ canonical_local_content ‖ occurrence)
//!
//! ## Why (CR-83)
//!
//! The previous derivation (CR-12 / CR-47) was **positional inside a
//! source-hash-scoped namespace**:
//!
//!   node_id = UUIDv5( UUIDv5(BLAZEGRAPH_NS, "{source_sha}:{config_hash}"),
//!                     text_order )
//!
//! Two failure modes for *editable* content (md / docx):
//!   1. `source_sha256` scoped every node ID → any edit anywhere rotated
//!      the whole document's IDs.
//!   2. The node-local input was `text_order` → inserting one paragraph
//!      shifted every later node's ID.
//!
//! The content+breadcrumb key fixes both:
//!   - **Document-unique**: the bgraph stays a faithful, round-trippable
//!     `HashMap<NodeId, Node>`. Two byte-identical paragraphs under the
//!     same breadcrumb get distinct IDs via the `occurrence` index.
//!   - **Edit-stable**: a node keeps its ID as long as its own content +
//!     heading-path are unchanged. Editing one paragraph rotates only that
//!     node; inserting / reordering siblings rotates nobody; renaming a
//!     section rotates only that section's subtree (its descendants'
//!     breadcrumbs change).
//!
//! ## What stays elsewhere (CR-83 § "What this CR does NOT change")
//!
//! - `source_sha256` + `config_hash` remain *document* discriminators in
//!   `ParseProvenance` and the doc-level fence (and thus feed
//!   `graph_sha256`). They are no longer *node* scoping.
//! - Cross-corpus **content dedup** (identical content → one stored blob)
//!   is the *absolute* coordinate and lives in URD's content-addressed
//!   store — out of scope here. Blazegraph's bgraph is a unique-keyed
//!   faithful graph; making the parse-level ID content-addressed would
//!   collide two identical paragraphs and break the `graph_sha256`
//!   round-trip. See CR-83 and CR-47.
//!
//! Node IDs remain:
//! - Deterministic: identical (content, breadcrumb, occurrence) → identical ID.
//! - Document-unique: the occurrence index disambiguates collisions.
//! - Version-invariant: no parser-version input.
//! - Standard UUIDv5: compatible with any system expecting UUID format.

use crate::types::NodeId;
use uuid::Uuid;

/// Fixed namespace UUID for all Blazegraph node IDs.
/// Computed as UUIDv5(DNS, "blazegraph.io") = a6f4212f-b2b3-5e5f-a124-e4f54c8bc5f9
const BLAZEGRAPH_NS: Uuid = Uuid::from_bytes([
    0xa6, 0xf4, 0x21, 0x2f, 0xb2, 0xb3, 0x5e, 0x5f, 0xa1, 0x24, 0xe4, 0xf5, 0x4c, 0x8b, 0xc5, 0xf9,
]);

/// Unit separator (`US`, 0x1f): delimits breadcrumb crumbs from one
/// another and from the local content. Using a control byte (never
/// present in text content) keeps the key unambiguous without escaping.
const SEP_FIELD: u8 = 0x1f;

/// Record separator (`RS`, 0x1e): delimits the optional occurrence suffix
/// from the content. Only present when `occurrence > 0`, so the common
/// (unique) case has the same key bytes a naive `breadcrumb ‖ content`
/// would — occurrence is a pure additive disambiguator.
const SEP_OCCURRENCE: u8 = 0x1e;

/// Generates content-derived, edit-stable, document-unique node IDs.
///
/// CR-83: this type carries **no per-document state** — the keying inputs
/// (content, breadcrumb, occurrence) are passed per call. It is kept as a
/// zero-sized struct (rather than free functions) so call sites and the
/// `GraphBuilder` signature change minimally, and so a future keying
/// variant could carry config without touching every call site.
#[derive(Debug, Clone, Default)]
pub struct NodeIdGenerator;

impl NodeIdGenerator {
    /// Create a generator. CR-83 removed the `(source_hash, config_hash)`
    /// parameters: node IDs are no longer document-namespace-scoped.
    pub fn new() -> Self {
        Self
    }

    /// The **build-time placeholder** root node ID.
    ///
    /// CR-83 / DT-10 (option C): the document root's *persisted* ID is the
    /// content fingerprint of the whole document ([`Self::root_id_from_nodes`]),
    /// which can only be computed once every child ID is known. The builder
    /// seeds the graph with this stable, content-free placeholder, builds the
    /// children, then re-keys the root to `root_id_from_nodes`. This value is
    /// therefore an internal sentinel — it is never the final root ID.
    pub fn root_id(&self) -> NodeId {
        // Empty breadcrumb path, sentinel content, occurrence 0.
        Self::node_id(b"\x00root", &[], 0)
    }

    /// CR-83 / DT-10 (option C): the **document-root** node ID — the content
    /// fingerprint of the whole document, `UUIDv5(BLAZEGRAPH_NS, ‖ sorted node
    /// IDs)` over every non-root node.
    ///
    /// Why a fingerprint rather than a constant or a `source_sha256` scope:
    /// - **Derivable from the node set alone** (no source / provenance), so
    ///   URD can recompute or verify a document's identity from just its node
    ///   IDs — no re-parse through Blazegraph (DT-10's headline property).
    /// - **Reorder-stable**: the IDs are sorted, so reordering siblings (same
    ///   ID set) keeps the same root.
    /// - **Edit-granular**: rotates only when some node's identity changes
    ///   (content / breadcrumb / structure), *not* on cosmetic source-byte
    ///   edits (unlike a `source_sha256` root) — same granularity as the body.
    /// - **Collision-free across distinct content**; two byte-identical docs
    ///   share a root (a doc-level dedup signal). Document identity *across
    ///   revisions* is URD's `stable_doc_id`, not this.
    /// - Empty document (no child nodes) → `UUIDv5(BLAZEGRAPH_NS, [])`, one
    ///   well-defined degenerate root.
    ///
    /// Caveat (DT-10): the root is therefore the one node that rotates on
    /// *every* content edit — it is a per-revision document fingerprint, not a
    /// stable handle. That is intentional; stable cross-revision identity is
    /// URD's job.
    pub fn root_id_from_nodes(node_ids: &[NodeId]) -> NodeId {
        let mut sorted: Vec<&NodeId> = node_ids.iter().collect();
        sorted.sort();
        let mut key = Vec::with_capacity(sorted.len() * 16);
        for id in sorted {
            key.extend_from_slice(id.as_bytes());
        }
        Uuid::new_v5(&BLAZEGRAPH_NS, &key)
    }

    /// Derive a node ID from its canonical local content, breadcrumb path,
    /// and occurrence index.
    ///
    /// Key layout (CR-83):
    /// ```text
    ///   for crumb in breadcrumb_path: crumb_bytes ‖ 0x1f
    ///   canonical_local_content
    ///   if occurrence > 0: 0x1e ‖ occurrence.to_le_bytes()
    /// ```
    ///
    /// - `local_content` — the node's **own** canonical bytes (a leaf's
    ///   text; a section's heading text). **Not** subtree-inclusive: a
    ///   section ID does not depend on its children, so descendant edits
    ///   don't rotate it. The caller is responsible for passing the
    ///   canonical form (see `node_key` in `graphs::builder`).
    /// - `breadcrumb_path` — the node's ancestor-heading trail. Crumbs are
    ///   the section heading texts from root → parent (excluding the node's
    ///   own heading). To keep subtrees of duplicate-heading siblings
    ///   unique, the builder folds a parent's occurrence into the crumb it
    ///   contributes (see `graphs::builder`).
    /// - `occurrence` — a stable index (0-based) among nodes that share the
    ///   *same* `(breadcrumb_path, local_content)`. `0` for the first /
    ///   only such node; omitted from the key entirely when `0` so the
    ///   unique case is occurrence-free.
    pub fn node_id(local_content: &[u8], breadcrumb_path: &[&str], occurrence: u32) -> NodeId {
        let mut key = Vec::with_capacity(local_content.len() + 8 * breadcrumb_path.len() + 8);
        for crumb in breadcrumb_path {
            key.extend_from_slice(crumb.as_bytes());
            key.push(SEP_FIELD);
        }
        key.push(SEP_FIELD);
        key.extend_from_slice(local_content);
        if occurrence > 0 {
            key.push(SEP_OCCURRENCE);
            key.extend_from_slice(&occurrence.to_le_bytes());
        }
        Uuid::new_v5(&BLAZEGRAPH_NS, &key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic: identical (content, breadcrumb, occurrence) → identical ID.
    #[test]
    fn test_deterministic_ids() {
        let a = NodeIdGenerator::node_id(b"hello world", &["Intro"], 0);
        let b = NodeIdGenerator::node_id(b"hello world", &["Intro"], 0);
        assert_eq!(a, b);
    }

    /// Content is a discriminator: different content → different ID, same breadcrumb.
    #[test]
    fn test_content_discriminates() {
        let a = NodeIdGenerator::node_id(b"alpha", &["Intro"], 0);
        let b = NodeIdGenerator::node_id(b"beta", &["Intro"], 0);
        assert_ne!(a, b);
    }

    /// Breadcrumb is a discriminator: same content under different headings → different ID.
    #[test]
    fn test_breadcrumb_discriminates() {
        let a = NodeIdGenerator::node_id(b"see above", &["Chapter 1"], 0);
        let b = NodeIdGenerator::node_id(b"see above", &["Chapter 2"], 0);
        assert_ne!(a, b);
    }

    /// CR-83 kicker primitive: two byte-identical paragraphs under the same
    /// breadcrumb disambiguate purely by `occurrence`.
    #[test]
    fn test_occurrence_disambiguates_identical_siblings() {
        let first = NodeIdGenerator::node_id(b"duplicate", &["Sec"], 0);
        let second = NodeIdGenerator::node_id(b"duplicate", &["Sec"], 1);
        let third = NodeIdGenerator::node_id(b"duplicate", &["Sec"], 2);
        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_ne!(first, third);
    }

    /// occurrence == 0 is omitted from the key, so it equals the
    /// "no occurrence" derivation. This keeps the unique (common) case
    /// occurrence-free, exactly as CR-83 specifies.
    #[test]
    fn test_occurrence_zero_is_omitted() {
        // There's no separate "no occurrence" entry point, but the
        // contract is that occurrence 0 produces the same key bytes a
        // naive breadcrumb‖content would. We pin it indirectly: 0 differs
        // from every nonzero index, and is itself stable.
        let zero_a = NodeIdGenerator::node_id(b"x", &["S"], 0);
        let zero_b = NodeIdGenerator::node_id(b"x", &["S"], 0);
        assert_eq!(zero_a, zero_b);
        assert_ne!(zero_a, NodeIdGenerator::node_id(b"x", &["S"], 1));
    }

    /// The breadcrumb-path separator prevents the classic concatenation
    /// ambiguity: `["ab"], "c"` must NOT collide with `["a"], "bc"`.
    #[test]
    fn test_no_concatenation_ambiguity() {
        let a = NodeIdGenerator::node_id(b"c", &["ab"], 0);
        let b = NodeIdGenerator::node_id(b"bc", &["a"], 0);
        assert_ne!(a, b);
    }

    /// The build-time placeholder root (DT-10) is stable, content-free, and
    /// distinct from content nodes. (The *persisted* root is a fingerprint —
    /// see `test_root_id_from_nodes_properties`.)
    #[test]
    fn test_root_id_stable_and_distinct() {
        let gen = NodeIdGenerator::new();
        assert_eq!(gen.root_id(), NodeIdGenerator::new().root_id());
        // A content node that happens to have empty breadcrumbs and
        // sentinel-like content must not collide with root.
        assert_ne!(gen.root_id(), NodeIdGenerator::node_id(b"root", &[], 0));
        assert_ne!(gen.root_id(), NodeIdGenerator::node_id(b"Document", &[], 0));
    }

    /// CR-83 / DT-10 (option C): the persisted root is the content fingerprint
    /// of the node set — deterministic, reorder-stable (sorted internally),
    /// and sensitive to the set's contents.
    #[test]
    fn test_root_id_from_nodes_properties() {
        let a = NodeIdGenerator::node_id(b"alpha", &["S"], 0);
        let b = NodeIdGenerator::node_id(b"beta", &["S"], 0);
        let c = NodeIdGenerator::node_id(b"gamma", &["S"], 0);

        // Deterministic.
        assert_eq!(
            NodeIdGenerator::root_id_from_nodes(&[a, b, c]),
            NodeIdGenerator::root_id_from_nodes(&[a, b, c])
        );
        // Reorder-stable: input order does not matter (sorted internally).
        assert_eq!(
            NodeIdGenerator::root_id_from_nodes(&[a, b, c]),
            NodeIdGenerator::root_id_from_nodes(&[c, a, b])
        );
        // Content-sensitive: a different node set → a different root.
        assert_ne!(
            NodeIdGenerator::root_id_from_nodes(&[a, b]),
            NodeIdGenerator::root_id_from_nodes(&[a, b, c])
        );
        // Valid UUIDv5.
        assert_eq!(
            NodeIdGenerator::root_id_from_nodes(&[a, b, c]).get_version_num(),
            5
        );
        // Empty document → one well-defined root, distinct from the
        // build placeholder.
        let empty = NodeIdGenerator::root_id_from_nodes(&[]);
        assert_eq!(empty, NodeIdGenerator::root_id_from_nodes(&[]));
        assert_ne!(empty, NodeIdGenerator::new().root_id());
    }

    /// CR-83: node IDs no longer depend on source / config / parser
    /// version. There is no namespace parameter to vary.
    #[test]
    fn test_no_document_namespace() {
        // Same content+breadcrumb produces the same ID regardless of any
        // (now-absent) document context.
        let a = NodeIdGenerator::node_id(b"body text", &["H1", "H2"], 0);
        let b = NodeIdGenerator::node_id(b"body text", &["H1", "H2"], 0);
        assert_eq!(a, b);
    }

    #[test]
    fn test_ids_are_valid_uuid_v5() {
        let gen = NodeIdGenerator::new();
        assert_eq!(gen.root_id().get_version_num(), 5);
        assert_eq!(
            NodeIdGenerator::node_id(b"x", &["S"], 0).get_version_num(),
            5
        );
    }

    /// GOLDEN: pin the exact key bytes → exact UUID, so an accidental
    /// change to the keying (separator, field order, occurrence encoding)
    /// is caught. If this test must change, the wire format major must bump.
    #[test]
    fn test_golden_key_derivation() {
        // breadcrumb ["Intro"], content "Hello.", occurrence 0.
        // key = "Intro" 0x1f  0x1f  "Hello."
        let mut expected_key = Vec::new();
        expected_key.extend_from_slice(b"Intro");
        expected_key.push(0x1f);
        expected_key.push(0x1f);
        expected_key.extend_from_slice(b"Hello.");
        let expected = Uuid::new_v5(&BLAZEGRAPH_NS, &expected_key);
        assert_eq!(NodeIdGenerator::node_id(b"Hello.", &["Intro"], 0), expected);

        // With occurrence 1: append 0x1e ‖ 1u32-le.
        let mut expected_key1 = expected_key.clone();
        expected_key1.push(0x1e);
        expected_key1.extend_from_slice(&1u32.to_le_bytes());
        let expected1 = Uuid::new_v5(&BLAZEGRAPH_NS, &expected_key1);
        assert_eq!(
            NodeIdGenerator::node_id(b"Hello.", &["Intro"], 1),
            expected1
        );
    }
}
