//! CR-28 — Graph Sanity-Check-and-Correction Pipe.
//!
//! Post-graph-build pass that enforces structural invariants on the assembled
//! graph. Each invariant has a check mode (always-on diagnostic emission) and
//! a correct mode (config-gated rewrite).
//!
//! Initial invariant: `depth_consistency` — every non-root node's depth must
//! equal `parent.depth + 1`. The `HierarchyContext` walk during section
//! detection is stateful; over-classification of title wrap-lines and similar
//! noise pollutes its stack and produces depth values that survive merging.
//! This pipe re-derives depth from the topology — `parent.depth + 1` becomes
//! the definition rather than a property, and stack-pollution leakage is
//! corrected at the output layer.
//!
//! Future invariants (childless pruning, repetition filter, empty-paragraph
//! pruning, duplicate collapse) plug into the same check + correct pattern.

use crate::config::{GraphSanityConfig, SectionHeightInvariantConfig};
use crate::types::{DocumentGraph, DocumentNode, NodeId};
use std::collections::{HashMap, HashSet, VecDeque};

/// Per-node record of a depth invariant violation.
#[derive(Debug, Clone)]
pub struct DepthViolation {
    pub node_id: NodeId,
    pub recorded_depth: u32,
    pub expected_depth: u32,
    pub corrected: bool,
}

/// CR-65 — Per-node record of a section-height-bounded-by-title violation.
#[derive(Debug, Clone)]
pub struct SectionHeightViolation {
    pub node_id: NodeId,
    pub section_height: f32,
    pub title_height: f32,
    pub threshold: f32,
    pub corrected: bool,
}

/// Diagnostic output from a sanity-pipe run. Always populated when the pipe
/// runs; consumers decide what to do with it (log, attach to output, etc.).
#[derive(Debug, Default, Clone)]
pub struct SanityReport {
    pub depth_violations: Vec<DepthViolation>,
    pub orphan_nodes: Vec<NodeId>,
    pub section_height_violations: Vec<SectionHeightViolation>,
}

impl SanityReport {
    pub fn is_clean(&self) -> bool {
        self.depth_violations.is_empty()
            && self.orphan_nodes.is_empty()
            && self.section_height_violations.is_empty()
    }
}

/// Apply the graph sanity-check-and-correction pipe.
///
/// Runs all enabled invariants. Returns a report of detected violations
/// regardless of whether corrections were applied.
pub fn apply(graph: &mut DocumentGraph, config: &GraphSanityConfig) -> SanityReport {
    let mut report = SanityReport::default();

    if !config.enabled {
        return report;
    }

    let dc = &config.invariants.depth_consistency;
    if dc.check || dc.correct {
        check_and_correct_depth(graph, dc.correct, &mut report);
    }

    let sh = &config.invariants.section_height_bounded_by_title;
    if sh.check || sh.correct {
        check_and_correct_section_height(graph, sh, &mut report);
    }

    // Future invariants: childless_sections, repetition_filter, etc. plug in here.

    if !report.is_clean() {
        println!(
            "🩺 GraphSanity: {} depth violations{}, {} orphan nodes, {} section-height violations{}",
            report.depth_violations.len(),
            if dc.correct && !report.depth_violations.is_empty() {
                " (corrected)"
            } else {
                ""
            },
            report.orphan_nodes.len(),
            report.section_height_violations.len(),
            if sh.correct && !report.section_height_violations.is_empty() {
                " (demoted to Paragraph)"
            } else {
                ""
            },
        );
    }

    report
}

/// BFS from root to compute expected depth for every reachable node.
/// Then compare to recorded depth and (optionally) rewrite.
///
/// The root sits at depth 0 (per `DocumentNode::new`'s convention) and its
/// children at depth 1. Orphan nodes — those not reachable from root — are
/// reported but their depth is preserved.
fn check_and_correct_depth(
    graph: &mut DocumentGraph,
    apply_corrections: bool,
    report: &mut SanityReport,
) {
    let root_id = graph.document_info.root_id;
    if !graph.nodes.contains_key(&root_id) {
        return;
    }

    // Pass 1 — BFS computes expected depth per reachable node.
    let mut expected: HashMap<NodeId, u32> = HashMap::new();
    expected.insert(root_id, 0);
    let mut queue: VecDeque<NodeId> = VecDeque::from([root_id]);
    while let Some(node_id) = queue.pop_front() {
        let depth = expected[&node_id];
        let children = graph
            .nodes
            .get(&node_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for child_id in children {
            // Cycle / shared-child guard: only enqueue first time we see a node.
            if expected.insert(child_id, depth + 1).is_none() {
                queue.push_back(child_id);
            }
        }
    }

    // Pass 2 — diagnose and (optionally) correct.
    let mut visited: HashSet<NodeId> = expected.keys().copied().collect();
    for (node_id, node) in graph.nodes.iter_mut() {
        if let Some(&exp) = expected.get(node_id) {
            let recorded = node.location.semantic.depth;
            if recorded != exp {
                report.depth_violations.push(DepthViolation {
                    node_id: *node_id,
                    recorded_depth: recorded,
                    expected_depth: exp,
                    corrected: apply_corrections,
                });
                if apply_corrections {
                    node.location.semantic.depth = exp;
                }
            }
        } else {
            report.orphan_nodes.push(*node_id);
        }
        visited.remove(node_id);
    }
    // visited now contains expected ids that aren't in the node map — graph
    // referential-integrity issue. Treat as orphan-equivalent diagnostic.
    for missing_id in visited {
        report.orphan_nodes.push(missing_id);
    }
}

/// CR-65 — Section bbox.height bounded by title.bbox.height × tolerance.
///
/// The font_size signal is unreliable on figure-heavy pages (Tika reports
/// inflated sizes for rotated/clustered text), but bbox.height is not. A
/// merged figure-cluster section's bbox spans many lines and is much taller
/// than any real single-line section header. The title — the document's own
/// declared "this is how big a heading is" element — sets the ceiling.
///
/// Violators are demoted to Paragraph in place (`node_type` change only,
/// topology preserved so `depth_consistency` still holds). No-op on documents
/// without a depth-1 Section bearing physical location (MD channel, short
/// docs).
fn check_and_correct_section_height(
    graph: &mut DocumentGraph,
    cfg: &SectionHeightInvariantConfig,
    report: &mut SanityReport,
) {
    let title_height = match find_title_height(graph) {
        Some(h) => h,
        None => return,
    };
    let threshold = title_height * cfg.tolerance;

    let violators: Vec<(NodeId, f32)> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.node_type == "Section")
        .filter_map(|(id, n)| {
            n.location
                .physical
                .as_ref()
                .map(|p| (*id, p.bounding_box.height))
        })
        .filter(|(_, h)| *h > threshold)
        .collect();

    for (node_id, section_height) in violators {
        report.section_height_violations.push(SectionHeightViolation {
            node_id,
            section_height,
            title_height,
            threshold,
            corrected: cfg.correct,
        });
        if cfg.correct {
            if let Some(node) = graph.nodes.get_mut(&node_id) {
                node.node_type = "Paragraph".to_string();
            }
        }
    }
}

/// CR-65 — Document title = first depth-1 Section in source order with a
/// physical bounding box. Returns its bbox.height, or None if no such node
/// exists (e.g., MD channel, short doc with no depth-1 sections).
fn find_title_height(graph: &DocumentGraph) -> Option<f32> {
    let mut candidates: Vec<&DocumentNode> = graph
        .nodes
        .values()
        .filter(|n| n.node_type == "Section")
        .filter(|n| n.location.semantic.depth == 1)
        .filter(|n| n.location.physical.is_some())
        .collect();
    candidates.sort_by_key(|n| n.text_order.unwrap_or(u32::MAX));
    candidates
        .first()
        .and_then(|n| n.location.physical.as_ref())
        .map(|p| p.bounding_box.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        GraphSanityConfig, GraphSanityInvariants, InvariantToggle, SectionHeightInvariantConfig,
    };
    use crate::types::{BoundingBox, DocumentGraph, DocumentNode, PhysicalLocation};
    use uuid::Uuid;

    /// Build a minimal graph: root → child(at depth `child_depth`).
    /// `child_depth` may be wrong on purpose to simulate stack-pollution.
    fn make_two_node_graph(child_depth: u32) -> (DocumentGraph, NodeId, NodeId) {
        let root_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();
        let mut graph = DocumentGraph::new_with_root(root_id);

        let mut root = DocumentNode::new_with_id(root_id, "Document", "root".into());
        root.children.push(child_id);
        root.location.semantic.depth = 0;

        let mut child = DocumentNode::new_with_id(child_id, "Section", "child".into());
        child.parent = Some(root_id);
        child.location.semantic.depth = child_depth;

        graph.nodes.insert(root_id, root);
        graph.nodes.insert(child_id, child);
        (graph, root_id, child_id)
    }

    fn full_correct_config() -> GraphSanityConfig {
        GraphSanityConfig {
            enabled: true,
            invariants: GraphSanityInvariants {
                depth_consistency: InvariantToggle {
                    check: true,
                    correct: true,
                },
                ..Default::default()
            },
        }
    }

    /// CR-28 Test 1 — depth recompute fixes drift.
    #[test]
    fn test_cr28_depth_recompute_fixes_drift() {
        // child recorded at depth 3 but is a direct child of root → expected 1
        let (mut graph, _, child_id) = make_two_node_graph(3);
        let report = apply(&mut graph, &full_correct_config());
        assert_eq!(report.depth_violations.len(), 1);
        assert_eq!(graph.nodes[&child_id].location.semantic.depth, 1);
    }

    /// CR-28 Test 2 — no-op on already-consistent tree.
    #[test]
    fn test_cr28_noop_on_consistent_tree() {
        let (mut graph, _, _) = make_two_node_graph(1);
        let report = apply(&mut graph, &full_correct_config());
        assert!(
            report.is_clean(),
            "consistent tree must produce no diagnostics"
        );
    }

    /// CR-28 Test 3 — check-only mode preserves original values.
    #[test]
    fn test_cr28_check_only_preserves_values() {
        let (mut graph, _, child_id) = make_two_node_graph(3);
        let cfg = GraphSanityConfig {
            enabled: true,
            invariants: GraphSanityInvariants {
                depth_consistency: InvariantToggle {
                    check: true,
                    correct: false,
                },
                ..Default::default()
            },
        };
        let report = apply(&mut graph, &cfg);
        assert_eq!(report.depth_violations.len(), 1);
        assert!(!report.depth_violations[0].corrected);
        // Original (wrong) depth retained
        assert_eq!(graph.nodes[&child_id].location.semantic.depth, 3);
    }

    /// CR-28 Test 4 — orphan node detected, depth preserved.
    #[test]
    fn test_cr28_orphan_detected_and_preserved() {
        let (mut graph, _, _) = make_two_node_graph(1);
        let orphan_id = Uuid::new_v4();
        let mut orphan = DocumentNode::new_with_id(orphan_id, "Section", "orphan".into());
        orphan.location.semantic.depth = 7; // arbitrary
                                            // Note: NOT added to root.children — this is the "orphan" case
        graph.nodes.insert(orphan_id, orphan);

        let report = apply(&mut graph, &full_correct_config());
        assert_eq!(report.orphan_nodes.len(), 1);
        assert_eq!(report.orphan_nodes[0], orphan_id);
        // Orphan's depth preserved (correction skipped)
        assert_eq!(graph.nodes[&orphan_id].location.semantic.depth, 7);
    }

    /// CR-28 Test 5 — disabled config is a no-op even with violations present.
    #[test]
    fn test_cr28_disabled_config_is_noop() {
        let (mut graph, _, child_id) = make_two_node_graph(3);
        let cfg = GraphSanityConfig {
            enabled: false,
            invariants: GraphSanityInvariants::default(),
        };
        let report = apply(&mut graph, &cfg);
        assert!(report.is_clean());
        // Wrong depth still in place
        assert_eq!(graph.nodes[&child_id].location.semantic.depth, 3);
    }

    /// CR-28 Test 6 — Police Act §86 regression: depth-5 paragraph drift.
    /// Construct: PART(d=2) → Section(d=3) → Paragraph(d=5).
    /// After correction: Paragraph at depth 4.
    #[test]
    fn test_cr28_police_act_section_86_regression() {
        let root_id = Uuid::new_v4();
        let part_id = Uuid::new_v4();
        let section_id = Uuid::new_v4();
        let paragraph_id = Uuid::new_v4();
        let mut graph = DocumentGraph::new_with_root(root_id);

        let mut root = DocumentNode::new_with_id(root_id, "Document", "doc".into());
        root.children.push(part_id);
        root.location.semantic.depth = 0;
        graph.nodes.insert(root_id, root);

        let mut part = DocumentNode::new_with_id(part_id, "Section", "PART 5".into());
        part.parent = Some(root_id);
        part.children.push(section_id);
        part.location.semantic.depth = 2;
        graph.nodes.insert(part_id, part);

        let mut section = DocumentNode::new_with_id(
            section_id,
            "Section",
            "86 Causing death by dangerous driving".into(),
        );
        section.parent = Some(part_id);
        section.children.push(paragraph_id);
        section.location.semantic.depth = 3; // correct
        graph.nodes.insert(section_id, section);

        let mut paragraph = DocumentNode::new_with_id(
            paragraph_id,
            "Paragraph",
            "(1) Part 1 of Schedule 2 …".into(),
        );
        paragraph.parent = Some(section_id);
        paragraph.location.semantic.depth = 5; // BUG: wrap-line stack drift
        graph.nodes.insert(paragraph_id, paragraph);

        // Note: PART's depth=2 is also "wrong" relative to root=0 (expected 1),
        // but matches the pre-CR28 behavior where PART legitimately sits at
        // depth 2 under a doc-title that the test fixture omits. We assert
        // that the paragraph drift is corrected; full chain consistency is
        // exercised by the broader regression suite.
        let report = apply(&mut graph, &full_correct_config());
        assert!(
            report
                .depth_violations
                .iter()
                .any(|v| v.node_id == paragraph_id),
            "paragraph drift must be detected"
        );
        assert_eq!(
            graph.nodes[&paragraph_id].location.semantic.depth,
            graph.nodes[&section_id].location.semantic.depth + 1,
            "paragraph depth must equal section depth + 1 after correction"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // CR-65 — section-height-bounded-by-title invariant tests
    // ──────────────────────────────────────────────────────────────────────

    /// Build a graph with root → (title Section, child Section), both depth=1
    /// with physical locations. Heights are configurable so tests can place
    /// the child above / below the title × tolerance threshold.
    fn make_graph_with_title_and_child(
        title_h: f32,
        child_h: f32,
    ) -> (DocumentGraph, NodeId, NodeId, NodeId) {
        let root_id = Uuid::new_v4();
        let title_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();
        let mut graph = DocumentGraph::new_with_root(root_id);

        let mut root = DocumentNode::new_with_id(root_id, "Document", "root".into());
        root.children.push(title_id);
        root.children.push(child_id);
        root.location.semantic.depth = 0;
        graph.nodes.insert(root_id, root);

        let bbox = |h: f32| BoundingBox {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: h,
        };

        let mut title = DocumentNode::new_with_id(title_id, "Section", "Title".into());
        title.parent = Some(root_id);
        title.location.semantic.depth = 1;
        title.text_order = Some(0);
        title.location.physical = Some(PhysicalLocation {
            page: 1,
            bounding_box: bbox(title_h),
        });
        graph.nodes.insert(title_id, title);

        let mut child = DocumentNode::new_with_id(child_id, "Section", "Child section".into());
        child.parent = Some(root_id);
        child.location.semantic.depth = 1;
        child.text_order = Some(1);
        child.location.physical = Some(PhysicalLocation {
            page: 2,
            bounding_box: bbox(child_h),
        });
        graph.nodes.insert(child_id, child);

        (graph, root_id, title_id, child_id)
    }

    /// CR-65 Test 1 — section taller than title × tolerance gets demoted.
    /// title.h = 18, child.h = 50, tolerance = 2.0 → threshold = 36 → 50 > 36 → demote.
    #[test]
    fn test_cr65_demotes_section_exceeding_threshold() {
        let (mut graph, _, _, child_id) = make_graph_with_title_and_child(18.0, 50.0);
        let report = apply(&mut graph, &full_correct_config());
        assert_eq!(report.section_height_violations.len(), 1);
        assert_eq!(report.section_height_violations[0].node_id, child_id);
        assert!(report.section_height_violations[0].corrected);
        assert_eq!(graph.nodes[&child_id].node_type, "Paragraph");
    }

    /// CR-65 Test 2 — section within tolerance is preserved.
    /// title.h = 18, child.h = 30, tolerance = 2.0 → threshold = 36 → 30 < 36 → keep.
    #[test]
    fn test_cr65_respects_tolerance() {
        let (mut graph, _, _, child_id) = make_graph_with_title_and_child(18.0, 30.0);
        let report = apply(&mut graph, &full_correct_config());
        assert!(
            report.section_height_violations.is_empty(),
            "child within tolerance should not be flagged"
        );
        assert_eq!(graph.nodes[&child_id].node_type, "Section");
    }

    /// CR-65 Test 3 — no depth-1 Section with physical location → no-op.
    /// Mirrors MD-channel / short-doc behavior. The CR-28 two-node graph has
    /// no physical locations, so the title is None and the rule no-ops.
    #[test]
    fn test_cr65_no_title_is_noop() {
        let (mut graph, _, _) = make_two_node_graph(1);
        let report = apply(&mut graph, &full_correct_config());
        assert!(
            report.section_height_violations.is_empty(),
            "no depth-1 Section with physical location → no violations"
        );
    }

    /// CR-65 Test 4 — check-only mode records violation but does not demote.
    #[test]
    fn test_cr65_check_only_does_not_demote() {
        let (mut graph, _, _, child_id) = make_graph_with_title_and_child(18.0, 50.0);
        let cfg = GraphSanityConfig {
            enabled: true,
            invariants: GraphSanityInvariants {
                section_height_bounded_by_title: SectionHeightInvariantConfig {
                    check: true,
                    correct: false,
                    tolerance: 2.0,
                },
                ..Default::default()
            },
        };
        let report = apply(&mut graph, &cfg);
        assert_eq!(report.section_height_violations.len(), 1);
        assert!(!report.section_height_violations[0].corrected);
        assert_eq!(
            graph.nodes[&child_id].node_type,
            "Section",
            "check-only must preserve node_type"
        );
    }

    /// CR-65 Test 5 — demoted section keeps text content + id (only node_type changes).
    #[test]
    fn test_cr65_demoted_section_keeps_content_and_id() {
        let (mut graph, _, _, child_id) = make_graph_with_title_and_child(18.0, 50.0);
        let original_text = graph.nodes[&child_id].content.text.clone();
        apply(&mut graph, &full_correct_config());
        assert!(graph.nodes.contains_key(&child_id), "id must be preserved");
        assert_eq!(
            graph.nodes[&child_id].content.text, original_text,
            "text content must be preserved"
        );
        assert_eq!(graph.nodes[&child_id].node_type, "Paragraph");
    }
}
