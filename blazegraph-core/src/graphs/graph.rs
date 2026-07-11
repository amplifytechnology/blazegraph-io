use super::analytics::GraphAnalytics;
use crate::types::*;
use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;

impl Default for DocumentGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl SortedDocumentGraph {
    /// Reconstruct the in-memory `DocumentGraph` (the content body) from
    /// this on-disk wrapper. Inverse of `to_sorted_graph`'s node
    /// projection: the envelope fields (`schema_version`, `created_at`,
    /// `parse_provenance`, `structural_profile`, `graph_sha256`) are
    /// dropped — none is part of identity.
    pub fn to_document_graph(&self) -> DocumentGraph {
        let nodes = self.nodes.iter().map(|n| (n.id, n.clone())).collect();
        DocumentGraph {
            nodes,
            document_info: self.document_info.clone(),
        }
    }

    /// Verify the embedded envelope `graph_sha256` against the hash
    /// recomputed from the reconstructed content body — the json-side
    /// analogue of the md parse path's identity check (Block C.3),
    /// producing the same [`ParseIdentity`] verdict under the same
    /// content-body hash. This is the json half of "always-on
    /// verification": a loaded graph.json can now prove it is untampered.
    ///
    /// An empty embedded hash (pre-Block-C graph.json fixtures, kept
    /// loadable via `#[serde(default)]`) yields `Verified` — there is no
    /// embedded value to contradict. Strict rejection on a `Derivative`
    /// verdict is the caller's to apply (mirroring the md path, where the
    /// CLI's compile-time `strict-identity` feature turns a non-`Verified`
    /// verdict into a hard reject).
    pub fn verify_identity(&self) -> crate::preprocessors::md::ParseIdentity {
        use crate::preprocessors::md::ParseIdentity;
        let recomputed =
            crate::graphs::serialization::canonical::graph_sha256(&self.to_document_graph());
        if self.graph_sha256.is_empty() || recomputed == self.graph_sha256 {
            ParseIdentity::Verified
        } else {
            ParseIdentity::Derivative {
                original_sha256: self.graph_sha256.clone(),
                recomputed_sha256: recomputed,
            }
        }
    }
}

impl DocumentGraph {
    /// Create a new graph with a deterministic root ID.
    pub fn new_with_root(root_id: NodeId) -> Self {
        use crate::types::{DocumentInfo, DocumentMetadata};

        let document_info = DocumentInfo {
            root_id,
            kind: crate::types::default_kind(),
            document_metadata: DocumentMetadata::default(),
            outline_data: None,
            flow_type: FlowType::default(),
            topology: None,
        };

        Self {
            nodes: HashMap::new(),
            document_info,
        }
    }

    /// Create a new graph with a random UUIDv4 root ID (legacy).
    pub fn new() -> Self {
        use crate::types::{DocumentInfo, DocumentMetadata};
        use uuid::Uuid;

        let document_info = DocumentInfo {
            root_id: Uuid::new_v4(),
            kind: crate::types::default_kind(),
            document_metadata: DocumentMetadata::default(),
            outline_data: None,
            flow_type: FlowType::default(),
            topology: None,
        };

        Self {
            nodes: HashMap::new(),
            document_info,
        }
    }

    pub fn max_depth(&self) -> u32 {
        self.nodes
            .values()
            .map(|n| n.location.semantic.depth)
            .max()
            .unwrap_or(0)
    }

    /// Serialize to graph.json on disk. `provenance` is threaded
    /// explicitly (Block A / Amendment M): it lands on the
    /// `SortedDocumentGraph` wrapper, outside the canonical hash.
    /// `None` for legacy build paths (random-UUIDv4 `build_graph`,
    /// stage dumps) that have no parse-run identity to record.
    pub fn save_to_json(&self, path: &str, provenance: Option<&ParseProvenance>) -> Result<()> {
        let sorted_graph = self.to_sorted_graph(provenance);
        let json = serde_json::to_string_pretty(&sorted_graph)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Project into the on-disk wrapper shape. Provenance is an explicit
    /// argument — `DocumentGraph` deliberately carries no provenance
    /// (content-not-provenance rule), so the wrapper is the only place
    /// the parse-run identity is written.
    pub fn to_sorted_graph(&self, provenance: Option<&ParseProvenance>) -> SortedDocumentGraph {
        // Collect all nodes and sort by text_order, with root node first
        let mut nodes: Vec<&DocumentNode> = self.nodes.values().collect();
        nodes.sort_by(|a, b| {
            // Document root (with text_order = None) should come first
            match (a.text_order, b.text_order) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(a_order), Some(b_order)) => a_order.cmp(&b_order),
            }
        });

        SortedDocumentGraph {
            // CR-87: json and md advertise the **one** serialization-
            // neutral schema/format version. `schema_version` here == the
            // md doc-level `schema` field == `BGRAPH_FORMAT_VERSION`.
            // Block C: stamped through the codec seam
            // (`FormatVersion::CURRENT`) — the same enum the md emitter
            // and the read path use — not a bare const. Wrapper field,
            // outside `graph_sha256`.
            schema_version: crate::graphs::serialization::version::FormatVersion::CURRENT
                .schema_str()
                .to_string(),
            // Block C.3: the json envelope carries `graph_sha256` so a
            // loaded graph.json is self-verifiable (symmetric with the
            // md doc-level block). It is an **envelope** field — outside
            // `canonical_json` / identity — computed by the same
            // recompute the md emitter uses, so its value *equals* the
            // md doc-level block's `graph_sha256` for the same graph and
            // does not move the content-body hash. See
            // `SortedDocumentGraph::verify_identity`.
            graph_sha256: crate::graphs::serialization::canonical::graph_sha256(self),
            // Wall-clock time at which this graph was serialized to disk.
            // Lives on the wrapper so `DocumentGraph` stays time-free —
            // see canonical-input invariant in
            // docs/P2/core/architecture/08-bgraph-md-format.md.
            created_at: Utc::now(),
            parse_provenance: provenance.cloned(),
            nodes: nodes.into_iter().cloned().collect(),
            document_info: self.document_info.clone(),
            // Json-only derived aggregate, recomputed at serialization
            // time (Block A / Amendment M) — never on `DocumentGraph`,
            // never hashed.
            structural_profile: self.compute_structural_profile(),
        }
    }

    /// Compute breadcrumbs for all nodes by walking the tree top-down.
    /// Sections contribute their text to the trail. Non-section nodes inherit
    /// their parent's breadcrumbs without adding to them.
    /// If document metadata has a title, it becomes the first breadcrumb.
    pub fn compute_breadcrumbs(&mut self) {
        let root_id = self.document_info.root_id;

        // Start with document title as first crumb if available
        let root_breadcrumbs: Vec<String> = self
            .document_info
            .document_metadata
            .title
            .as_ref()
            .filter(|t| !t.is_empty())
            .map(|t| vec![t.clone()])
            .unwrap_or_default();

        // Set breadcrumbs on the Document node itself
        if let Some(doc_node) = self.nodes.get_mut(&root_id) {
            doc_node.location.semantic.breadcrumbs = root_breadcrumbs.clone();
        }

        // Collect children to avoid borrow conflict
        let root_children: Vec<NodeId> = self
            .nodes
            .get(&root_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();

        for child_id in root_children {
            self.propagate_breadcrumbs(child_id, &root_breadcrumbs);
        }
    }

    /// Recursively propagate breadcrumbs down the tree
    fn propagate_breadcrumbs(&mut self, node_id: NodeId, parent_breadcrumbs: &[String]) {
        // Determine this node's breadcrumbs
        let (node_breadcrumbs, children) = {
            let node = match self.nodes.get(&node_id) {
                Some(n) => n,
                None => return,
            };

            let breadcrumbs = if node.node_type == "Section" {
                // Sections contribute their text to the trail
                let mut crumbs = parent_breadcrumbs.to_vec();
                crumbs.push(node.content.text.clone());
                crumbs
            } else {
                // Non-sections inherit parent breadcrumbs
                parent_breadcrumbs.to_vec()
            };

            (breadcrumbs, node.children.clone())
        };

        // Set breadcrumbs on this node
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.location.semantic.breadcrumbs = node_breadcrumbs.clone();
        }

        // Recurse into children
        for child_id in children {
            self.propagate_breadcrumbs(child_id, &node_breadcrumbs);
        }
    }

    /// Analyze any subtree starting from given node
    pub fn _analyze_subtree(&self, root_node_id: NodeId) -> Option<GraphAnalyticsResult> {
        let subtree_nodes = self._collect_subtree_nodes(root_node_id);
        if subtree_nodes.is_empty() {
            return None;
        }
        Some(GraphAnalytics::compute_analytics(&subtree_nodes))
    }

    /// Collect all nodes in a subtree starting from given root
    fn _collect_subtree_nodes(&self, root_node_id: NodeId) -> Vec<&DocumentNode> {
        let mut subtree_nodes = Vec::new();

        if let Some(root_node) = self.nodes.get(&root_node_id) {
            self._collect_subtree_recursive(root_node, &mut subtree_nodes);
        }

        subtree_nodes
    }

    /// Recursively collect all nodes in subtree
    fn _collect_subtree_recursive<'a>(
        &'a self,
        node: &'a DocumentNode,
        collected: &mut Vec<&'a DocumentNode>,
    ) {
        collected.push(node);

        for child_id in &node.children {
            if let Some(child_node) = self.nodes.get(child_id) {
                self._collect_subtree_recursive(child_node, collected);
            }
        }
    }
}
