use crate::types::*;
use anyhow::Result;
use std::collections::HashMap;
use super::analytics::GraphAnalytics;

impl DocumentGraph {
    pub fn new() -> Self {
        use uuid::Uuid;
        use crate::types::{DocumentMetadata, DocumentAnalysis, DocumentRootNode};
        
        // Create a default root node - this will be properly populated during graph building
        let root_node = DocumentRootNode {
            id: Uuid::new_v4(),
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
            children: Vec::new(),
        };

        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            root_node,
            metadata: GraphMetadata::default(),
        }
    }

    pub fn max_depth(&self) -> u32 {
        self.nodes.values().map(|n| n.depth).max().unwrap_or(0)
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
            edges: self.edges.values().cloned().collect(),
            root_node: self.root_node.clone(),
            metadata: self.metadata.clone(),
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