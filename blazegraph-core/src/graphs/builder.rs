use super::node_id::NodeIdGenerator;
use crate::types::*;
use anyhow::Result;

pub struct GraphBuilder;

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self
    }

    /// Build a document graph with deterministic node IDs.
    ///
    /// The `NodeIdGenerator` produces UUIDv5 IDs scoped to a specific
    /// `(version, source_hash, config_hash)` triple, ensuring identical
    /// inputs always produce identical graphs.
    ///
    /// Channel contract: `elements` arrives at this boundary fully
    /// transformed — no merge/reorder/post-process happens here. We
    /// walk-and-zip: each `SemanticTreeElement` becomes one
    /// `DocumentNode`, with a deterministic ID salted from
    /// `text_order` (which equals vec index, asserted in debug builds).
    ///
    /// `parse_provenance` is persisted on
    /// `graph.document_info.parse_provenance` so downstream consumers
    /// (notably the bgraph.md emitter) can reproduce the graph and
    /// emit round-trippable identity fields without re-deriving them.
    /// The same `(blazegraph_version, source_sha256, config_hash)`
    /// triple feeds `id_gen`; callers should construct
    /// `ParseProvenance` from the same data.
    pub fn build_graph_deterministic(
        &self,
        elements: Vec<SemanticTreeElement>,
        id_gen: &NodeIdGenerator,
        parse_provenance: ParseProvenance,
    ) -> Result<DocumentGraph> {
        println!(
            "🏗️  Building document graph from {} elements",
            elements.len()
        );

        let mut graph = DocumentGraph::new_with_root(id_gen.root_id());
        let mut node_stack: Vec<NodeId> = Vec::new();

        let root_id = graph.document_info.root_id;
        node_stack.push(root_id);

        // Create the Document root node
        let document_node = DocumentNode {
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
            children: Vec::new(),
        };
        graph.nodes.insert(root_id, document_node);

        for (index, element) in elements.iter().enumerate() {
            // Sanity-check: the channel projection must assign
            // `text_order = vec_index`. Drift here would mean a
            // post-projection reorder snuck in.
            debug_assert_eq!(
                element.text_order as usize, index,
                "text_order/vec-position drift: element[{}].text_order = {}",
                index, element.text_order
            );
            let node_id = id_gen.node_id(element.text_order);

            let node = self.create_node(element, element.text_order, node_id);

            // Determine parent based on hierarchy level
            let parent_id = self.find_parent(&mut node_stack, element.hierarchy_level, root_id);

            // Insert node and create relationships
            let mut final_node = node;
            final_node.parent = Some(parent_id);
            final_node.location.semantic.depth = element.hierarchy_level;
            final_node.text_order = Some(element.text_order);
            final_node.location.semantic.path =
                self.generate_hierarchical_path(&graph, parent_id, index);

            graph.nodes.insert(node_id, final_node);

            // Update parent's children list
            if let Some(parent) = graph.nodes.get_mut(&parent_id) {
                parent.children.push(node_id);
            }

            // Update hierarchy stack for sections
            if matches!(element.element_type, SemanticElementType::Section) {
                while let Some(&stack_id) = node_stack.last() {
                    if let Some(stack_node) = graph.nodes.get(&stack_id) {
                        if stack_node.location.semantic.depth >= element.hierarchy_level {
                            node_stack.pop();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                node_stack.push(node_id);
            }
        }

        // Update structural profile node count
        graph.structural_profile.total_nodes = graph.nodes.len();
        graph.structural_profile.document_type = DocumentType::Generic;

        // Persist origin so the bgraph.md emitter (B2) and any future
        // round-trip consumer can reproduce the graph deterministically
        // without re-reading the source bytes.
        graph.document_info.parse_provenance = Some(parse_provenance);

        println!("✅ Graph built: {} nodes", graph.nodes.len());

        Ok(graph)
    }

    /// Build a document graph with random UUIDv4 IDs (legacy fallback).
    /// Used by --dump-stages and other paths that don't have source/config
    /// hashes.
    pub fn build_graph(&self, elements: Vec<SemanticTreeElement>) -> Result<DocumentGraph> {
        println!(
            "🏗️  Building document graph from {} elements",
            elements.len()
        );

        let mut graph = DocumentGraph::new();
        let mut node_stack: Vec<NodeId> = Vec::new();

        let root_id = graph.document_info.root_id;
        node_stack.push(root_id);

        // Create the Document root node
        let document_node = DocumentNode {
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
            children: Vec::new(),
        };
        graph.nodes.insert(root_id, document_node);

        for (index, element) in elements.iter().enumerate() {
            let node = self.create_node_v4(element, index as u32);
            let node_id = node.id;

            let parent_id = self.find_parent(&mut node_stack, element.hierarchy_level, root_id);

            let mut final_node = node;
            final_node.parent = Some(parent_id);
            final_node.location.semantic.depth = element.hierarchy_level;
            final_node.text_order = Some(index as u32);
            final_node.location.semantic.path =
                self.generate_hierarchical_path(&graph, parent_id, index);

            graph.nodes.insert(node_id, final_node);

            if let Some(parent) = graph.nodes.get_mut(&parent_id) {
                parent.children.push(node_id);
            }

            if matches!(element.element_type, SemanticElementType::Section) {
                while let Some(&stack_id) = node_stack.last() {
                    if let Some(stack_node) = graph.nodes.get(&stack_id) {
                        if stack_node.location.semantic.depth >= element.hierarchy_level {
                            node_stack.pop();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                node_stack.push(node_id);
            }
        }

        graph.structural_profile.total_nodes = graph.nodes.len();
        graph.structural_profile.document_type = DocumentType::Generic;

        println!("✅ Graph built: {} nodes", graph.nodes.len());

        Ok(graph)
    }

    fn find_parent(&self, node_stack: &mut Vec<NodeId>, level: u32, root_id: NodeId) -> NodeId {
        if level <= 1 {
            node_stack.truncate(1);
            root_id
        } else {
            while node_stack.len() > level as usize {
                node_stack.pop();
            }
            node_stack.last().copied().unwrap_or(root_id)
        }
    }

    fn generate_hierarchical_path(
        &self,
        graph: &DocumentGraph,
        parent_id: NodeId,
        index: usize,
    ) -> String {
        if parent_id == graph.document_info.root_id {
            let child_count = graph
                .nodes
                .get(&parent_id)
                .map(|n| n.children.len())
                .unwrap_or(0);
            format!("{}", child_count + 1)
        } else if let Some(parent) = graph.nodes.get(&parent_id) {
            format!(
                "{}.{}",
                parent.location.semantic.path,
                parent.children.len() + 1
            )
        } else {
            format!("{}", index + 1)
        }
    }

    /// Create node with deterministic ID (UUIDv5).
    fn create_node(
        &self,
        element: &SemanticTreeElement,
        order: u32,
        node_id: NodeId,
    ) -> DocumentNode {
        let node_type_str = node_type_for(element.element_type);
        let mut node = DocumentNode::new_with_id(node_id, node_type_str, element.text.clone());
        node.location.physical = element.physical_location.clone();
        node.text_order = Some(order);
        node.token_count = element.token_count;
        node.style_info = element.style.clone();
        node
    }

    /// Create node with random ID (UUIDv4, legacy).
    fn create_node_v4(&self, element: &SemanticTreeElement, order: u32) -> DocumentNode {
        let node_type_str = node_type_for(element.element_type);
        let mut node = DocumentNode::new(node_type_str, element.text.clone());
        node.location.physical = element.physical_location.clone();
        node.text_order = Some(order);
        node.token_count = element.token_count;
        node.style_info = element.style.clone();
        node
    }
}

/// Map `SemanticElementType` to the string the serialized graph carries
/// in `DocumentNode.node_type`. Kept as a free function (no `&self`) so
/// it doesn't pretend to depend on `GraphBuilder` state.
fn node_type_for(t: SemanticElementType) -> &'static str {
    match t {
        SemanticElementType::Section => "Section",
        SemanticElementType::Paragraph => "Paragraph",
        SemanticElementType::Header => "Header",
        SemanticElementType::Footer => "Footer",
        SemanticElementType::Margin => "Margin",
    }
}
