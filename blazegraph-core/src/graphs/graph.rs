use crate::types::*;
use anyhow::Result;
use std::collections::HashMap;
use super::analytics::GraphAnalytics;

impl DocumentGraph {
    pub fn new() -> Self {
        use uuid::Uuid;
        use crate::types::{DocumentMetadata, DocumentAnalysis, DocumentInfo};

        // Create default document info — will be populated during graph building
        let document_info = DocumentInfo {
            root_id: Uuid::new_v4(),
            document_metadata: DocumentMetadata::default(),
            document_analysis: DocumentAnalysis {
                font_size_counts: std::collections::HashMap::new(),
                font_family_counts: std::collections::HashMap::new(),
                bold_counts: (0, 0),
                italic_counts: (0, 0),
                most_common_font_size: 12.0,
                most_common_font_family: "unknown".to_string(),
                all_font_sizes: Vec::new(),
            },
        };

        Self {
            nodes: HashMap::new(),
            document_info,
            metadata: GraphMetadata::default(),
        }
    }

    pub fn max_depth(&self) -> u32 {
        self.nodes.values().map(|n| n.location.semantic.depth).max().unwrap_or(0)
    }

    pub fn save_to_json(&self, path: &str) -> Result<()> {
        let sorted_graph = self.to_sorted_graph();
        let json = serde_json::to_string_pretty(&sorted_graph)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn to_sorted_graph(&self) -> SortedDocumentGraph {
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
            nodes: nodes.into_iter().cloned().collect(),
            document_info: self.document_info.clone(),
            metadata: self.metadata.clone(),
        }
    }

    /// Compute breadcrumbs for all nodes by walking the tree top-down.
    /// Sections contribute their text to the trail. Non-section nodes inherit
    /// their parent's breadcrumbs without adding to them.
    /// If document metadata has a title, it becomes the first breadcrumb.
    pub fn compute_breadcrumbs(&mut self) {
        let root_id = self.document_info.root_id;

        // Start with document title as first crumb if available
        let root_breadcrumbs: Vec<String> = self.document_info.document_metadata.title
            .as_ref()
            .filter(|t| !t.is_empty())
            .map(|t| vec![t.clone()])
            .unwrap_or_default();
        
        // Set breadcrumbs on the Document node itself
        if let Some(doc_node) = self.nodes.get_mut(&root_id) {
            doc_node.location.semantic.breadcrumbs = root_breadcrumbs.clone();
        }
        
        // Collect children to avoid borrow conflict
        let root_children: Vec<NodeId> = self.nodes
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
    fn _collect_subtree_recursive<'a>(&'a self, node: &'a DocumentNode, collected: &mut Vec<&'a DocumentNode>) {
        collected.push(node);
        
        for child_id in &node.children {
            if let Some(child_node) = self.nodes.get(child_id) {
                self._collect_subtree_recursive(child_node, collected);
            }
        }
    }
}