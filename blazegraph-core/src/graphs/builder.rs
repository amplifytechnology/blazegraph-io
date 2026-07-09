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

    /// Build a document graph with deterministic, content-derived node IDs.
    ///
    /// CR-83: each node's ID is `UUIDv5(BLAZEGRAPH_NS, breadcrumb ‖
    /// content ‖ occurrence)` — document-unique and edit-stable. There is
    /// no `(source_hash, config_hash)` document namespace anymore
    /// (`source_sha256` / `config_hash` stay as *document* discriminators
    /// in `ParseProvenance` and `graph_sha256`, not as node scoping).
    ///
    /// Channel contract: `elements` arrives at this boundary fully
    /// transformed — no merge/reorder/post-process happens here. We
    /// walk-and-zip: each `SemanticTreeElement` becomes one
    /// `DocumentNode`. `text_order` **stays as a node field** (ordering /
    /// emission) but is no longer an ID input — inserting or reordering
    /// siblings no longer rotates IDs.
    ///
    /// Breadcrumb derivation is done *inline here* (not deferred to
    /// `compute_breadcrumbs`) because the node ID needs the ancestor-
    /// heading path at construction time. The result matches
    /// `compute_breadcrumbs` for section ancestry; the document-title
    /// prefix that `compute_breadcrumbs` adds is intentionally *not* part
    /// of the ID key (it is a document-constant, so it cannot affect
    /// within-document uniqueness, and folding it in would couple every
    /// node ID to the title — an edit that should be document-level only).
    ///
    /// Provenance is deliberately **not** a parameter and **not**
    /// stamped onto the graph (Block A / Amendment M): `DocumentGraph`
    /// is the canonical-hash input and carries only content. The build
    /// path holds its `ParseProvenance` as a plain value and threads it
    /// explicitly into the emit/serialize calls that need it.
    pub fn build_graph_deterministic(
        &self,
        elements: Vec<SemanticTreeElement>,
        id_gen: &NodeIdGenerator,
    ) -> Result<DocumentGraph> {
        eprintln!(
            "🏗️  Building document graph from {} elements",
            elements.len()
        );

        let mut graph = DocumentGraph::new_with_root(id_gen.root_id());
        let mut node_stack: Vec<NodeId> = Vec::new();

        // CR-83: breadcrumb stack parallel to the section ancestry on
        // `node_stack`. Each entry is the ID-key crumb a section
        // contributes — its canonical heading text with that section's own
        // `occurrence` folded in (open-detail #1: this is what keeps the
        // *descendants* of duplicate-heading siblings unique, since
        // children inherit the parent's crumb verbatim).
        let mut breadcrumb_stack: Vec<String> = Vec::new();

        // CR-83: occurrence ledger. Maps a node's `(breadcrumb_path,
        // local_content)` signature to how many nodes with that exact
        // signature have already been emitted. The Nth such node takes
        // occurrence = N (0-based), so byte-identical siblings under the
        // same breadcrumb disambiguate.
        let mut occurrence_ledger: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();

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
            internal_refs: vec![],
            external_refs: vec![],
            confidence: 0,
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

            let is_section = matches!(element.element_type, SemanticElementType::Section);

            // Determine parent based on hierarchy level. This settles the
            // section ancestry on `node_stack` (a section pops shallower-
            // or-equal sections; a leaf truncates to the current section).
            let parent_id = self.find_parent(&mut node_stack, element.hierarchy_level, root_id);

            // CR-83: keep `breadcrumb_stack` in lock-step with the section
            // ancestry now present on `node_stack`. `find_parent` may have
            // popped sections; drop their crumbs so the breadcrumb path
            // reflects only live ancestors. (`node_stack[0]` is the root,
            // which contributes no crumb, hence the `len() - 1`.)
            breadcrumb_stack.truncate(node_stack.len().saturating_sub(1));

            // The node's own ID-key inputs.
            let local_content = canonical_local_content(element);
            let breadcrumb_path: Vec<&str> = breadcrumb_stack.iter().map(|s| s.as_str()).collect();
            let occurrence =
                next_occurrence(&mut occurrence_ledger, &breadcrumb_path, &local_content);
            let node_id =
                NodeIdGenerator::node_id(local_content.as_bytes(), &breadcrumb_path, occurrence);

            let node = self.create_node(element, element.text_order, node_id);

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

            // Update hierarchy stack for sections.
            if is_section {
                while let Some(&stack_id) = node_stack.last() {
                    if let Some(stack_node) = graph.nodes.get(&stack_id) {
                        if stack_node.location.semantic.depth >= element.hierarchy_level {
                            node_stack.pop();
                            breadcrumb_stack.pop();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                node_stack.push(node_id);
                // CR-83 (open-detail #1): the crumb this section contributes
                // to its descendants folds in *its own* occurrence. Two
                // `### Example` siblings push distinct crumbs ("Example" vs
                // "Example\u{1e}1"), so their children — which inherit the
                // crumb verbatim — get distinct breadcrumb paths and thus
                // distinct IDs even when their bodies are byte-identical.
                breadcrumb_stack.push(fold_occurrence(&local_content, occurrence));
            }
        }

        // CR-83 / DT-10 (option C): re-key the document root from the
        // finished node set. We built under a content-free placeholder
        // (`id_gen.root_id()`) because child IDs aren't known until the loop
        // runs; the root's *persisted* ID is the content fingerprint of all
        // non-root nodes — `UUIDv5(BLAZEGRAPH_NS, ‖ sorted node IDs)`. This
        // makes the root derivable from the node set alone (URD can verify a
        // doc's identity without re-parsing) and rotate exactly when the
        // document's content/structure changes — never on a cosmetic edit.
        // The root is the only `parent: None` node, so the re-key is local:
        // swap its key, then every top-level child's `parent` pointer.
        let placeholder_root = root_id;
        let child_ids: Vec<NodeId> = graph
            .nodes
            .keys()
            .filter(|id| **id != placeholder_root)
            .copied()
            .collect();
        let final_root = NodeIdGenerator::root_id_from_nodes(&child_ids);
        if final_root != placeholder_root {
            let mut root_node = graph
                .nodes
                .remove(&placeholder_root)
                .expect("placeholder root node present");
            root_node.id = final_root;
            for node in graph.nodes.values_mut() {
                if node.parent == Some(placeholder_root) {
                    node.parent = Some(final_root);
                }
            }
            graph.nodes.insert(final_root, root_node);
            graph.document_info.root_id = final_root;
        }

        // Update structural profile node count
        graph.structural_profile.total_nodes = graph.nodes.len();
        graph.structural_profile.document_type = DocumentType::Generic;

        eprintln!("✅ Graph built: {} nodes", graph.nodes.len());

        Ok(graph)
    }

    /// Build a document graph with random UUIDv4 IDs (legacy fallback).
    /// Used by --dump-stages and other paths that don't have source/config
    /// hashes.
    pub fn build_graph(&self, elements: Vec<SemanticTreeElement>) -> Result<DocumentGraph> {
        eprintln!(
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
            internal_refs: vec![],
            external_refs: vec![],
            confidence: 0,
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

        eprintln!("✅ Graph built: {} nodes", graph.nodes.len());

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
        // CR-62: thread internal/external refs from the channel-projected
        // SemanticTreeElement through to the in-graph DocumentNode so the
        // bgraph.md emitter can serialize them per the v2.3.0 schema.
        node.internal_refs = element.internal_refs.clone();
        node.external_refs = element.external_refs.clone();
        // CR-78 (v2.4.0): thread detection confidence through to the node so
        // the bgraph.md emitter can serialize it.
        node.confidence = element.confidence;
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
        // CR-62: see above.
        node.internal_refs = element.internal_refs.clone();
        node.external_refs = element.external_refs.clone();
        // CR-78: see above.
        node.confidence = element.confidence;
        node
    }
}

/// CR-83: occurrence-fold separator. Identical to the node-ID key's
/// occurrence separator (`RS`, 0x1e) so a folded crumb is unambiguous and
/// never collides with a real heading byte. Only appended when the
/// section's own occurrence is non-zero — the first / unique occurrence
/// contributes a verbatim crumb, keeping the common case fold-free.
const OCCURRENCE_FOLD_SEP: char = '\u{1e}';

/// CR-83: the canonical **local content** bytes for a node's ID key.
///
/// Open-detail #2 decision: the key is the node's `text` **verbatim**, with
/// **no `node_type` component** and **no extra whitespace normalization**.
///
/// - *No `node_type`*: the breadcrumb path already separates sections
///   (which contribute crumbs) from leaves (which inherit them), and the
///   occurrence index resolves true content collisions. Adding `node_type`
///   would couple a node's ID to its classification — a later
///   reclassification that preserves text would needlessly rotate the ID.
/// - *Canonical node bytes, not raw*: the key uses exactly the bytes that
///   live on the node and survive emit→parse — i.e. `NodeContent::new`'s
///   form (reserved-prefix escaped + trimmed), NOT the raw `element.text`.
///   Deriving from the raw text rotates the ID across the escape round-trip
///   when a body line carries a reserved fence prefix (the self-referential
///   case): the emitter writes the escaped bytes, so the parser re-derives
///   from those. Escaping/trimming is idempotent, so build (from raw) and
///   parse (from already-escaped) converge on the same key. (C-7 canonical
///   form: `08-bgraph-md-format.md` § C-7.)
fn canonical_local_content(element: &SemanticTreeElement) -> String {
    crate::types::NodeContent::new(element.text.clone()).text
}

/// CR-83: signature → next occurrence index for a node, mutating the ledger.
///
/// The signature is the node's `(breadcrumb_path, local_content)` — every
/// ID-key input *except* the occurrence itself. The first node with a given
/// signature gets `0` (occurrence-free key); the Nth gets `N`. Crumbs are
/// joined with the same `US` (0x1f) byte the ID key uses, so two distinct
/// breadcrumb paths can never produce the same signature string.
fn next_occurrence(
    ledger: &mut std::collections::HashMap<String, u32>,
    breadcrumb_path: &[&str],
    local_content: &str,
) -> u32 {
    let mut sig = String::new();
    for crumb in breadcrumb_path {
        sig.push_str(crumb);
        sig.push('\u{1f}');
    }
    sig.push('\u{1f}');
    sig.push_str(local_content);
    let counter = ledger.entry(sig).or_insert(0);
    let occurrence = *counter;
    *counter += 1;
    occurrence
}

/// CR-83: the crumb a section contributes to its descendants' breadcrumb
/// path, with the section's own `occurrence` folded in (open-detail #1).
/// Occurrence `0` → verbatim heading; occurrence `N>0` → `heading␞N`.
fn fold_occurrence(local_content: &str, occurrence: u32) -> String {
    if occurrence == 0 {
        local_content.to_string()
    } else {
        format!("{local_content}{OCCURRENCE_FOLD_SEP}{occurrence}")
    }
}

/// Map `SemanticElementType` to the string the serialized graph carries
/// in `DocumentNode.node_type`. Kept as a free function (no `&self`) so
/// it doesn't pretend to depend on `GraphBuilder` state.
pub fn node_type_for(t: SemanticElementType) -> &'static str {
    match t {
        SemanticElementType::Section => "Section",
        SemanticElementType::Paragraph => "Paragraph",
        SemanticElementType::Header => "Header",
        SemanticElementType::Footer => "Footer",
        SemanticElementType::Margin => "Margin",
        // Schema 0.7.0+ (B6): markdown-channel block types.
        SemanticElementType::CodeBlock => "CodeBlock",
        SemanticElementType::List => "List",
        SemanticElementType::Blockquote => "Blockquote",
        SemanticElementType::Table => "Table",
        // CR-59: `Message` is an orphan variant with no in-memory
        // production path (see `SemanticElementType::Message` doc
        // comment). Reaching this arm means some new code path
        // constructed a Message element on the shared tree-topology
        // type — that's the regression the orphan-sentinel design
        // guards against. Panic loudly so it's caught immediately.
        SemanticElementType::Message => panic!(
            "SemanticElementType::Message is an orphan variant with no \
             tree-topology production path; see types.rs for the future \
             stream-topology design slice."
        ),
    }
}
