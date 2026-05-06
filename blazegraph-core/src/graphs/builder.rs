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
    /// The NodeIdGenerator produces UUIDv5 IDs scoped to a specific
    /// (version, pdf_hash, config_hash) triple, ensuring identical inputs
    /// always produce identical graphs.
    pub fn build_graph_deterministic(
        &self,
        elements: Vec<ParsedPdfElement>,
        id_gen: &NodeIdGenerator,
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

        // Group elements into meaningful chunks
        let grouped_elements = self.group_elements_into_chunks(elements);
        println!(
            "📦 Grouped {} elements into {} meaningful chunks",
            grouped_elements
                .iter()
                .map(|g| g.elements.len())
                .sum::<usize>(),
            grouped_elements.len()
        );

        for (index, group) in grouped_elements.iter().enumerate() {
            let text_order = index as u32;
            let node_id = id_gen.node_id(text_order);

            let node = self.create_node_from_group(group, text_order, node_id)?;

            // Determine parent based on hierarchy level
            let parent_id = self.find_parent(&mut node_stack, group.hierarchy_level, root_id);

            // Insert node and create relationships
            let mut final_node = node;
            final_node.parent = Some(parent_id);
            final_node.location.semantic.depth = group.hierarchy_level;
            final_node.text_order = Some(text_order);
            final_node.location.semantic.path =
                self.generate_hierarchical_path(&graph, parent_id, index);

            graph.nodes.insert(node_id, final_node);

            // Update parent's children list
            if let Some(parent) = graph.nodes.get_mut(&parent_id) {
                parent.children.push(node_id);
            }

            // Update hierarchy stack for sections
            if matches!(group.group_type, GroupType::Section) {
                while let Some(&stack_id) = node_stack.last() {
                    if let Some(stack_node) = graph.nodes.get(&stack_id) {
                        if stack_node.location.semantic.depth >= group.hierarchy_level {
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

        println!("✅ Graph built: {} nodes", graph.nodes.len());

        Ok(graph)
    }

    /// Build a document graph with random UUIDv4 IDs (legacy fallback).
    /// Used by --dump-stages and other paths that don't have pdf/config hashes.
    pub fn build_graph(&self, elements: Vec<ParsedPdfElement>) -> Result<DocumentGraph> {
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

        let grouped_elements = self.group_elements_into_chunks(elements);
        println!(
            "📦 Grouped {} elements into {} meaningful chunks",
            grouped_elements
                .iter()
                .map(|g| g.elements.len())
                .sum::<usize>(),
            grouped_elements.len()
        );

        for (index, group) in grouped_elements.iter().enumerate() {
            let node = self.create_node_from_group_v4(group, index as u32)?;
            let node_id = node.id;

            let parent_id = self.find_parent(&mut node_stack, group.hierarchy_level, root_id);

            let mut final_node = node;
            final_node.parent = Some(parent_id);
            final_node.location.semantic.depth = group.hierarchy_level;
            final_node.text_order = Some(index as u32);
            final_node.location.semantic.path =
                self.generate_hierarchical_path(&graph, parent_id, index);

            graph.nodes.insert(node_id, final_node);

            if let Some(parent) = graph.nodes.get_mut(&parent_id) {
                parent.children.push(node_id);
            }

            if matches!(group.group_type, GroupType::Section) {
                while let Some(&stack_id) = node_stack.last() {
                    if let Some(stack_node) = graph.nodes.get(&stack_id) {
                        if stack_node.location.semantic.depth >= group.hierarchy_level {
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

    fn group_elements_into_chunks(&self, elements: Vec<ParsedPdfElement>) -> Vec<ElementGroup> {
        let mut groups = Vec::new();

        for element in elements.iter() {
            let group_type = match element.element_type {
                crate::types::ParsedElementType::Section => GroupType::Section,
                crate::types::ParsedElementType::List => GroupType::Paragraph,
                crate::types::ParsedElementType::ListItem => GroupType::Paragraph,
                crate::types::ParsedElementType::Paragraph => GroupType::Paragraph,
                crate::types::ParsedElementType::Header => GroupType::Header,
                crate::types::ParsedElementType::Footer => GroupType::Footer,
                crate::types::ParsedElementType::Margin => GroupType::Margin,
            };

            groups.push(ElementGroup {
                elements: vec![element.clone()],
                group_type,
                hierarchy_level: element.hierarchy_level,
                combined_text: element.text.clone(),
            });
        }

        groups
    }

    /// Create node with deterministic ID (UUIDv5)
    fn create_node_from_group(
        &self,
        group: &ElementGroup,
        order: u32,
        node_id: NodeId,
    ) -> Result<DocumentNode> {
        let (node_type_str, physical) = self.extract_node_type_and_physical(group);

        let mut node =
            DocumentNode::new_with_id(node_id, node_type_str, group.combined_text.clone());
        node.location.physical = physical;
        node.text_order = Some(order);
        node.token_count = group.elements.iter().map(|e| e.token_count).sum();
        self.apply_style_info(&mut node, group);

        Ok(node)
    }

    /// Create node with random ID (UUIDv4, legacy)
    fn create_node_from_group_v4(&self, group: &ElementGroup, order: u32) -> Result<DocumentNode> {
        let (node_type_str, physical) = self.extract_node_type_and_physical(group);

        let mut node = DocumentNode::new(node_type_str, group.combined_text.clone());
        node.location.physical = physical;
        node.text_order = Some(order);
        node.token_count = group.elements.iter().map(|e| e.token_count).sum();
        self.apply_style_info(&mut node, group);

        Ok(node)
    }

    fn extract_node_type_and_physical(
        &self,
        group: &ElementGroup,
    ) -> (&str, Option<PhysicalLocation>) {
        if let Some(first_element) = group.elements.first() {
            let node_type = match first_element.element_type {
                crate::types::ParsedElementType::Section => "Section",
                crate::types::ParsedElementType::List => "List",
                crate::types::ParsedElementType::ListItem => "ListItem",
                crate::types::ParsedElementType::Paragraph => "Paragraph",
                crate::types::ParsedElementType::Header => "Header",
                crate::types::ParsedElementType::Footer => "Footer",
                crate::types::ParsedElementType::Margin => "Margin",
            };

            let placement = first_element.pdf_placement();
            let physical = Some(PhysicalLocation {
                page: placement.page_number,
                bounding_box: placement.bounding_box.clone(),
            });

            (node_type, physical)
        } else {
            let node_type = match group.group_type {
                GroupType::Section => "Section",
                GroupType::Paragraph => "Paragraph",
                GroupType::Header => "Header",
                GroupType::Footer => "Footer",
                GroupType::Margin => "Margin",
            };
            (node_type, None)
        }
    }

    fn apply_style_info(&self, node: &mut DocumentNode, group: &ElementGroup) {
        if let Some(first_element) = group.elements.first() {
            node.style_info = Some(StyleMetadata {
                font_class: first_element.style_info.class_name.clone(),
                font_size: Some(first_element.style_info.font_size),
                font_family: Some(first_element.style_info.font_family.clone()),
                color: Some(first_element.style_info.color.clone()),
                is_bold: first_element
                    .style_info
                    .font_weight
                    .to_lowercase()
                    .contains("bold"),
                is_italic: first_element
                    .style_info
                    .font_style
                    .to_lowercase()
                    .contains("italic"),
            });
        }
    }
}
