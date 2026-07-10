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

        // CR-83 / CR-84: derive every body node's ID up front via the
        // shared ID-derivation walk. The walk is the single home of the
        // breadcrumb-stack + occurrence-ledger logic — the same function
        // re-keys a settled graph post-sanity ([`rekey_node_ids`]), so the
        // build-time and finalize-time derivations cannot drift.
        let canonical_contents: Vec<String> =
            elements.iter().map(canonical_local_content).collect();
        let node_ids = derive_walk_ids(elements.iter().zip(canonical_contents.iter()).map(
            |(element, content)| IdWalkRow {
                is_section: matches!(element.element_type, SemanticElementType::Section),
                hierarchy_level: element.hierarchy_level,
                local_content: content,
            },
        ));

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

            // CR-84: the ID comes from the shared walk's pre-pass (same
            // index order as this loop — both iterate `elements` as-is).
            let node_id = node_ids[index];

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

            // Update hierarchy stack for sections. (The parallel
            // breadcrumb bookkeeping lives in `derive_walk_ids` — CR-84.)
            if is_section {
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
        // Block A / A3: `element.confidence` deliberately does NOT flow
        // onto the node — DocumentNode carries content only; the CR-78
        // signal stays parser-internal (sidecar into graph_sanity).
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
        // Block A / A3: see above — confidence stays off the node.
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

// =========================================================================
// CR-84: the shared ID-derivation walk + the post-sanity re-key pass.
// =========================================================================

/// One row of the shared ID-derivation walk (CR-84): everything the
/// derivation needs to know about one body node. Rows are consumed in
/// `text_order` (emission) order.
///
/// Both adapters produce the identical row stream for the same logical
/// document:
/// - the **build** path projects each `SemanticTreeElement`
///   (`hierarchy_level`, `canonical_local_content`), and
/// - the **re-key** path projects each settled `DocumentNode`
///   (`location.semantic.depth`, `content.text` — already the
///   `NodeContent::new` canonical form).
struct IdWalkRow<'a> {
    is_section: bool,
    hierarchy_level: u32,
    local_content: &'a str,
}

/// CR-84: the single home of the CR-83 ID-derivation algorithm — the
/// breadcrumb stack, the occurrence ledger, and the
/// [`NodeIdGenerator::node_id`] call — as a pure function of the row
/// stream. Returns one ID per row, in row order.
///
/// The stack discipline mirrors the builder's topology walk exactly
/// (`find_parent` truncation + the section pop/push), driven purely by
/// `hierarchy_level`, so the IDs this derives from a settled graph's
/// stored depths equal the IDs the builder derives when it rebuilds that
/// graph from the same `(node_type, depth, text_order, content)`
/// projection — the round-trip invariant holds by construction.
///
/// The walk is a pure function of the rows, so running it twice over the
/// same settled topology reproduces identical IDs (idempotence — the
/// property that keeps the post-sanity re-key a no-op on MD/DOCX and
/// clean-PDF graphs whose topology sanity never moved).
fn derive_walk_ids<'a>(rows: impl Iterator<Item = IdWalkRow<'a>>) -> Vec<NodeId> {
    // `stack_depths` mirrors the builder's `node_stack`, holding each
    // live ancestor's depth. Entry 0 is the Document root (depth 0),
    // which contributes no crumb — `breadcrumb_stack` runs parallel to
    // `stack_depths[1..]`.
    let mut stack_depths: Vec<u32> = vec![0];
    let mut breadcrumb_stack: Vec<String> = Vec::new();
    let mut occurrence_ledger: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut ids: Vec<NodeId> = Vec::new();

    for row in rows {
        // `find_parent` equivalent: settle the ancestry for this node.
        if row.hierarchy_level <= 1 {
            stack_depths.truncate(1);
        } else {
            while stack_depths.len() > row.hierarchy_level as usize {
                stack_depths.pop();
            }
        }
        // Keep the crumbs in lock-step with the live ancestry (the root
        // contributes no crumb, hence the `len() - 1`).
        breadcrumb_stack.truncate(stack_depths.len().saturating_sub(1));

        // The node's own ID-key inputs (CR-83 key layout).
        let breadcrumb_path: Vec<&str> = breadcrumb_stack.iter().map(|s| s.as_str()).collect();
        let occurrence =
            next_occurrence(&mut occurrence_ledger, &breadcrumb_path, row.local_content);
        ids.push(NodeIdGenerator::node_id(
            row.local_content.as_bytes(),
            &breadcrumb_path,
            occurrence,
        ));

        // Section re-stacking, mirroring the builder's post-insert loop:
        // pop ancestors at the same-or-deeper depth, then push this
        // section's depth and its crumb. CR-83 (open-detail #1): the crumb
        // folds in the section's *own* occurrence, so descendants of
        // duplicate-heading siblings inherit distinct breadcrumb paths.
        if row.is_section {
            while let Some(&depth) = stack_depths.last() {
                if depth >= row.hierarchy_level {
                    stack_depths.pop();
                    breadcrumb_stack.pop();
                } else {
                    break;
                }
            }
            stack_depths.push(row.hierarchy_level);
            breadcrumb_stack.push(fold_occurrence(row.local_content, occurrence));
        }
    }
    ids
}

/// CR-84: finalize node identity from a **settled** graph's topology.
///
/// > Node identity is finalized as late as possible — after every
/// > topology-mutating pass — and derived from the *final emitted
/// > structure* (the post-sanity breadcrumb ancestry).
///
/// `graph_sanity` re-parents / re-depths nodes but (pre-CR-84) never
/// re-keyed them, so the forward path emitted IDs derived from the
/// builder's PRE-sanity ancestry inside a POST-sanity tree — the reverse
/// parser then honestly re-derived different IDs from the emitted
/// structure. This pass re-runs the shared derivation walk
/// ([`derive_walk_ids`]) over the settled topology and remaps the graph
/// onto the finalized IDs, generalizing the DT-10 build-provisional →
/// re-key-when-final root pattern to the whole node set.
///
/// Remapped: the `nodes` HashMap keys, every `DocumentNode.id`, every
/// `parent` / `children` pointer, and `DocumentInfo.root_id` (recomputed
/// via [`NodeIdGenerator::root_id_from_nodes`] from the finalized IDs).
/// `InternalRef` / `ExternalRef` targets are `Named`/`Page`/`Uri` — they
/// carry no `NodeId`, so refs need no remap.
///
/// Derivation inputs come from the node's **section-heading ancestry as
/// the walk reconstructs it from stored depths** — NOT from the stored
/// `location.breadcrumbs`, which `compute_breadcrumbs` prefixes with the
/// document title (a document-constant that is intentionally not part of
/// the ID key — see the note on `build_graph_deterministic`).
///
/// **Also finalizes `location.semantic.path`** (CR-84, found during
/// implementation): `path` is the *other* build-time structural
/// derivation (1-based child index joined by `.`) that `graph_sanity`
/// mutates out from under — on attention.pdf, 103 stored paths diverged
/// from what the reverse parse re-derives, independently of the 89 IDs
/// (pre-existing at v4, masked by the ID divergence in the Block A
/// diagnosis). It is hashed content, so the round-trip contract requires
/// it derivable from the emitted structure too. Recomputed top-down from
/// the settled tree with the builder's exact rule; runs unconditionally
/// (paths can shift even when no ID moves — sibling index shifts).
///
/// Idempotent: on a graph whose IDs and paths already match its settled
/// topology (MD/DOCX builds, clean PDFs, an already-re-keyed graph) the
/// walk reproduces the same values and the pass is a no-op. Returns the
/// number of node IDs and stored paths that moved (both 0 for the no-op
/// case).
pub fn rekey_node_ids(graph: &mut DocumentGraph) -> RekeyOutcome {
    // Project the settled graph into the walk's row order: body nodes
    // (text_order = Some) ascending. The Document root (text_order =
    // None) is not walked — its ID is the node-set fingerprint (DT-10).
    let mut body: Vec<&DocumentNode> = graph
        .nodes
        .values()
        .filter(|n| n.text_order.is_some())
        .collect();
    body.sort_by_key(|n| n.text_order.expect("filtered to Some above"));

    let new_ids = derive_walk_ids(body.iter().map(|node| IdWalkRow {
        is_section: node.node_type == "Section",
        hierarchy_level: node.location.semantic.depth,
        local_content: &node.content.text,
    }));

    let old_root = graph.document_info.root_id;
    let new_root = NodeIdGenerator::root_id_from_nodes(&new_ids);

    let mut mapping: std::collections::HashMap<NodeId, NodeId> = body
        .iter()
        .map(|n| n.id)
        .zip(new_ids.iter().copied())
        .collect();
    mapping.insert(old_root, new_root);

    let ids_moved = mapping.iter().filter(|(old, new)| old != new).count();
    if ids_moved > 0 {
        let remap = |id: &NodeId| -> NodeId {
            *mapping
                .get(id)
                .unwrap_or_else(|| panic!("rekey_node_ids: dangling node reference {id}"))
        };

        let old_nodes = std::mem::take(&mut graph.nodes);
        let mut new_nodes: std::collections::HashMap<NodeId, DocumentNode> =
            std::collections::HashMap::with_capacity(old_nodes.len());
        for (old_id, mut node) in old_nodes {
            node.id = remap(&old_id);
            node.parent = node.parent.as_ref().map(&remap);
            for child in node.children.iter_mut() {
                *child = remap(child);
            }
            let clobbered = new_nodes.insert(node.id, node);
            debug_assert!(
                clobbered.is_none(),
                "rekey_node_ids: two nodes remapped onto one ID — the occurrence \
                 ledger guarantees walk-unique IDs, so this is a bug"
            );
        }
        graph.nodes = new_nodes;
        graph.document_info.root_id = new_root;
    }

    let paths_moved = finalize_paths(graph);
    RekeyOutcome {
        ids_moved,
        paths_moved,
    }
}

/// What [`rekey_node_ids`] did: how many node IDs were re-keyed and how
/// many stored `location.semantic.path` values were re-derived. Both `0`
/// ⇔ the graph's topology was already settled (the idempotent no-op).
#[derive(Debug, Clone, Copy, Default)]
pub struct RekeyOutcome {
    pub ids_moved: usize,
    pub paths_moved: usize,
}

/// CR-84: re-derive every node's `location.semantic.path` from the
/// settled tree, using the builder's exact rule
/// (`generate_hierarchical_path`): a node's path is its parent's path
/// plus `.` plus its 1-based index among the parent's children (root
/// children are the bare index; the Document root keeps its empty
/// path). Returns how many stored paths changed.
fn finalize_paths(graph: &mut DocumentGraph) -> usize {
    let root_id = graph.document_info.root_id;
    let mut changed = 0usize;
    let mut queue: std::collections::VecDeque<(NodeId, String)> = graph
        .nodes
        .get(&root_id)
        .map(|root| {
            root.children
                .iter()
                .enumerate()
                .map(|(i, child)| (*child, format!("{}", i + 1)))
                .collect()
        })
        .unwrap_or_default();

    while let Some((id, path)) = queue.pop_front() {
        let Some(node) = graph.nodes.get_mut(&id) else {
            debug_assert!(false, "finalize_paths: dangling child reference {id}");
            continue;
        };
        if node.location.semantic.path != path {
            node.location.semantic.path = path.clone();
            changed += 1;
        }
        for (i, child) in node.children.iter().enumerate() {
            queue.push_back((*child, format!("{path}.{}", i + 1)));
        }
    }
    changed
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `SemanticTreeElement` stream for the deterministic builder.
    /// Rows are `(node_type, text, hierarchy_level)`; `text_order` is the
    /// row index (the channel contract).
    fn elements(rows: &[(&str, &str, u32)]) -> Vec<SemanticTreeElement> {
        rows.iter()
            .enumerate()
            .map(|(i, (node_type, text, level))| SemanticTreeElement {
                text: text.to_string(),
                element_type: match *node_type {
                    "Section" => SemanticElementType::Section,
                    "Paragraph" => SemanticElementType::Paragraph,
                    other => panic!("unsupported test node type {other:?}"),
                },
                hierarchy_level: *level,
                text_order: i as u32,
                physical_location: None,
                style: None,
                token_count: 1,
                internal_refs: vec![],
                external_refs: vec![],
                confidence: 0,
            })
            .collect()
    }

    fn build(rows: &[(&str, &str, u32)]) -> DocumentGraph {
        GraphBuilder::new()
            .build_graph_deterministic(elements(rows), &NodeIdGenerator::new())
            .expect("graph builds")
    }

    /// `text_order → node id` — the lens the CR-84 assertions use.
    fn ids_by_order(g: &DocumentGraph) -> std::collections::HashMap<u32, NodeId> {
        g.nodes
            .values()
            .filter_map(|n| n.text_order.map(|t| (t, n.id)))
            .collect()
    }

    /// `text_order → semantic path` — for the path-finalize assertions.
    fn paths_by_order(g: &DocumentGraph) -> std::collections::HashMap<u32, String> {
        g.nodes
            .values()
            .filter_map(|n| {
                n.text_order
                    .map(|t| (t, n.location.semantic.path.clone()))
            })
            .collect()
    }

    /// CR-84 idempotence: on a graph whose topology is exactly what the
    /// builder produced (the settled case — MD/DOCX, clean PDFs, and the
    /// reverse-parse path), the re-key derives identical IDs and paths
    /// and is a no-op. This is the property that bounds the v5
    /// node-canon impact to sanity-mutated documents.
    #[test]
    fn rekey_is_noop_on_settled_topology() {
        let mut g = build(&[
            ("Section", "Intro", 1),
            ("Paragraph", "Hello.", 2),
            ("Section", "Body", 1),
            ("Paragraph", "World.", 2),
        ]);
        let before_ids = ids_by_order(&g);
        let before_paths = paths_by_order(&g);
        let before_root = g.document_info.root_id;

        let outcome = rekey_node_ids(&mut g);

        assert_eq!(outcome.ids_moved, 0, "settled topology: no ID moves");
        assert_eq!(outcome.paths_moved, 0, "settled topology: no path moves");
        assert_eq!(ids_by_order(&g), before_ids);
        assert_eq!(paths_by_order(&g), before_paths);
        assert_eq!(g.document_info.root_id, before_root);
    }

    /// CR-84 core contract: after a topology mutation (what
    /// `graph_sanity` does — re-parent + re-depth), the re-key derives
    /// exactly the IDs a fresh build of the *mutated* shape derives —
    /// i.e. the IDs the reverse parser will re-derive from the emitted
    /// (post-sanity) tree. Forward-post-sanity == reverse-derived, by
    /// construction.
    #[test]
    fn rekey_after_mutation_matches_fresh_derivation_of_settled_shape() {
        // Built shape: A(1) > B(2) > P(3).
        let mut g = build(&[
            ("Section", "A", 1),
            ("Section", "B", 2),
            ("Paragraph", "P.", 3),
        ]);

        // Simulate a sanity rebalance: B is re-depthed to a top-level
        // sibling of A (depth 2 → 1), P follows (depth 3 → 2). Re-parent
        // B under the root, exactly as CR-70 would.
        let root_id = g.document_info.root_id;
        let a_id = g
            .nodes
            .values()
            .find(|n| n.content.text == "A")
            .unwrap()
            .id;
        let b_id = g
            .nodes
            .values()
            .find(|n| n.content.text == "B")
            .unwrap()
            .id;
        let p_id = g
            .nodes
            .values()
            .find(|n| n.content.text == "P.")
            .unwrap()
            .id;
        g.nodes.get_mut(&b_id).unwrap().parent = Some(root_id);
        g.nodes.get_mut(&b_id).unwrap().location.semantic.depth = 1;
        g.nodes.get_mut(&p_id).unwrap().location.semantic.depth = 2;
        g.nodes.get_mut(&a_id).unwrap().children.retain(|c| *c != b_id);
        g.nodes.get_mut(&root_id).unwrap().children.push(b_id);

        let outcome = rekey_node_ids(&mut g);
        assert!(
            outcome.ids_moved > 0,
            "a real topology mutation must move IDs"
        );

        // The settled shape, built fresh (this is what the reverse
        // parser does with the emitted depths).
        let fresh = build(&[
            ("Section", "A", 1),
            ("Section", "B", 1),
            ("Paragraph", "P.", 2),
        ]);

        assert_eq!(
            ids_by_order(&g),
            ids_by_order(&fresh),
            "re-keyed IDs must equal a fresh derivation from the settled topology"
        );
        assert_eq!(
            paths_by_order(&g),
            paths_by_order(&fresh),
            "finalized paths must equal a fresh derivation from the settled topology"
        );
        assert_eq!(
            g.document_info.root_id, fresh.document_info.root_id,
            "root fingerprint must be recomputed from the finalized IDs"
        );

        // "A" kept its ID (its own ancestry never moved); "B" and "P."
        // rotated (their breadcrumb ancestry changed).
        assert!(g.nodes.contains_key(&a_id), "unmoved node keeps its ID");
        assert!(!g.nodes.contains_key(&b_id), "re-depthed section rotates");
        assert!(!g.nodes.contains_key(&p_id), "its descendant rotates");

        // Structural pointers were remapped consistently: every parent /
        // child edge resolves, and the root is the only parentless node.
        for node in g.nodes.values() {
            if let Some(p) = node.parent {
                assert!(g.nodes.contains_key(&p), "parent pointer resolves");
            } else {
                assert_eq!(node.id, g.document_info.root_id);
            }
            for c in &node.children {
                assert!(g.nodes.contains_key(c), "child pointer resolves");
                assert_eq!(g.nodes[c].parent, Some(node.id));
            }
        }

        // Idempotence: a second re-key of the now-finalized graph is a
        // no-op.
        let second = rekey_node_ids(&mut g);
        assert_eq!(second.ids_moved, 0, "re-key must be idempotent (ids)");
        assert_eq!(second.paths_moved, 0, "re-key must be idempotent (paths)");
    }

    /// DT-10 generalization: after a re-key the root is the fingerprint
    /// of the finalized (non-root) node set.
    #[test]
    fn rekey_recomputes_root_fingerprint_from_final_ids() {
        let mut g = build(&[("Section", "S", 1), ("Paragraph", "Text.", 2)]);
        // Mutate: paragraph hoisted to top level (sanity-style).
        let s_id = g
            .nodes
            .values()
            .find(|n| n.content.text == "S")
            .unwrap()
            .id;
        let t_id = g
            .nodes
            .values()
            .find(|n| n.content.text == "Text.")
            .unwrap()
            .id;
        let root_id = g.document_info.root_id;
        g.nodes.get_mut(&t_id).unwrap().parent = Some(root_id);
        g.nodes.get_mut(&t_id).unwrap().location.semantic.depth = 1;
        g.nodes.get_mut(&s_id).unwrap().children.retain(|c| *c != t_id);
        g.nodes.get_mut(&root_id).unwrap().children.push(t_id);

        rekey_node_ids(&mut g);

        let body_ids: Vec<NodeId> = g
            .nodes
            .values()
            .filter_map(|n| n.text_order.map(|_| n.id))
            .collect();
        assert_eq!(
            g.document_info.root_id,
            NodeIdGenerator::root_id_from_nodes(&body_ids),
            "root must be the fingerprint of the finalized node set"
        );
    }
}
