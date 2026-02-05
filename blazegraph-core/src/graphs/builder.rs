use crate::types::*;
use anyhow::Result;
use uuid::Uuid;
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

    /// Build graph from elements and populate root node with metadata and analysis
    pub fn build_graph_with_metadata(
        &self,
        elements: Vec<ParsedPdfElement>,
        document_metadata: DocumentMetadata,
        document_analysis: DocumentAnalysis,
    ) -> Result<DocumentGraph> {
        let mut graph = self.build_graph(elements)?;

        // Update root node with proper metadata and analysis
        graph.root_node.document_metadata = document_metadata;
        graph.root_node.document_analysis = document_analysis;

        Ok(graph)
    }

    pub fn build_graph(&self, elements: Vec<ParsedPdfElement>) -> Result<DocumentGraph> {
        println!(
            "🏗️  Building document graph from {} elements",
            elements.len()
        );

        let mut graph = DocumentGraph::new();
        let mut node_stack: Vec<NodeId> = Vec::new(); // Track hierarchy

        // The root node is already created in DocumentGraph::new()
        // We just need to track its ID for building the hierarchy
        let root_id = graph.root_node.id;
        node_stack.push(root_id);

        // Create a Document node in the nodes HashMap that mirrors the root_node
        // This allows the frontend to find and render the document as a visual node
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
            text_order: None, // Document comes first (None sorts before Some)
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
            let node = self.create_node_from_group(group, index as u32)?;
            let node_id = node.id;

            // Determine parent based on hierarchy level
            let parent_id = self.find_parent(&mut node_stack, group.hierarchy_level, root_id);

            // Insert node and create relationships
            let mut final_node = node;
            final_node.parent = Some(parent_id);
            final_node.location.semantic.depth = group.hierarchy_level;
            final_node.text_order = Some(index as u32);
            final_node.location.semantic.path =
                self.generate_hierarchical_path(&graph, parent_id, index);

            graph.nodes.insert(node_id, final_node);

            // Update parent's children list
            if parent_id == root_id {
                // If parent is root node, update both root_node and the Document node in nodes
                graph.root_node.children.push(node_id);
                // Also update the Document node in the nodes HashMap
                if let Some(doc_node) = graph.nodes.get_mut(&root_id) {
                    doc_node.children.push(node_id);
                }
            } else if let Some(parent) = graph.nodes.get_mut(&parent_id) {
                parent.children.push(node_id);
            }

            // Create edges
            self.create_edge(&mut graph, parent_id, node_id, EdgeType::Child);
            self.create_edge(&mut graph, node_id, parent_id, EdgeType::Parent);

            // Update hierarchy stack for sections
            if matches!(group.group_type, GroupType::Section) {
                // Remove items at same or higher level
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

        // Post-processing: compute breadcrumbs from final tree structure
        graph.compute_breadcrumbs();

        // Update metadata
        graph.metadata.total_nodes = graph.nodes.len();
        graph.metadata.document_type = DocumentType::Generic; // Will be updated by processor

        println!(
            "✅ Graph built: {} nodes, {} edges",
            graph.nodes.len(),
            graph.edges.len()
        );

        Ok(graph)
    }

    fn find_parent(&self, node_stack: &mut Vec<NodeId>, level: u32, root_id: NodeId) -> NodeId {
        if level <= 1 {
            // Top level - parent is root
            node_stack.truncate(1);
            root_id
        } else {
            // Find appropriate parent based on hierarchy level
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
        if parent_id == graph.root_node.id {
            // Parent is root node - this is a top-level section
            format!("{}", graph.root_node.children.len() + 1)
        } else if let Some(parent) = graph.nodes.get(&parent_id) {
            // Build path from parent's path
            format!(
                "{}.{}",
                parent.location.semantic.path,
                parent.children.len() + 1
            )
        } else {
            format!("{}", index + 1)
        }
    }

    fn create_edge(
        &self,
        graph: &mut DocumentGraph,
        source: NodeId,
        target: NodeId,
        edge_type: EdgeType,
    ) {
        let edge = DocumentEdge {
            id: Uuid::new_v4(),
            source,
            target,
            edge_type,
        };
        graph.edges.insert(edge.id, edge);
    }

    fn group_elements_into_chunks(&self, elements: Vec<ParsedPdfElement>) -> Vec<ElementGroup> {
        let mut groups = Vec::new();

        // Simple 1:1 mapping - create one ElementGroup per ParsedElement
        for element in elements.iter() {
            let group_type = match element.element_type {
                crate::types::ParsedElementType::Section => GroupType::Section,
                crate::types::ParsedElementType::List => GroupType::Paragraph, // Lists are content like paragraphs
                crate::types::ParsedElementType::ListItem => GroupType::Paragraph, // ListItems are content like paragraphs
                crate::types::ParsedElementType::Paragraph => GroupType::Paragraph,
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

    fn create_node_from_group(&self, group: &ElementGroup, order: u32) -> Result<DocumentNode> {
        // Determine node type from the first ParsedElement
        let (node_type_str, physical) = if let Some(first_element) = group.elements.first() {
            let node_type = match first_element.element_type {
                crate::types::ParsedElementType::Section => "Section",
                crate::types::ParsedElementType::List => "List",
                crate::types::ParsedElementType::ListItem => "ListItem",
                crate::types::ParsedElementType::Paragraph => "Paragraph",
            };

            // Build PhysicalLocation from ParsedElement's flat fields
            let physical = Some(PhysicalLocation {
                page: first_element.page_number,
                bounding_box: first_element.bounding_box.clone(),
            });

            (node_type, physical)
        } else {
            let node_type = match group.group_type {
                GroupType::Section => "Section",
                GroupType::Paragraph => "Paragraph",
            };
            (node_type, None)
        };

        let mut node = DocumentNode::new(node_type_str, group.combined_text.clone());
        node.location.physical = physical;
        node.text_order = Some(order);
        node.token_count = group.elements.iter().map(|e| e.token_count).sum();

        // Style info from the most prominent element
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

        Ok(node)
    }
}
