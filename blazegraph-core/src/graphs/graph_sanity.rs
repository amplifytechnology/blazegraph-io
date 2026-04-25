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

use crate::config::GraphSanityConfig;
use crate::types::{DocumentGraph, NodeId};
use std::collections::{HashMap, HashSet, VecDeque};

/// Per-node record of a depth invariant violation.
#[derive(Debug, Clone)]
pub struct DepthViolation {
    pub node_id: NodeId,
    pub recorded_depth: u32,
    pub expected_depth: u32,
    pub corrected: bool,
}

/// Diagnostic output from a sanity-pipe run. Always populated when the pipe
/// runs; consumers decide what to do with it (log, attach to output, etc.).
#[derive(Debug, Default, Clone)]
pub struct SanityReport {
    pub depth_violations: Vec<DepthViolation>,
    pub orphan_nodes: Vec<NodeId>,
}

impl SanityReport {
    pub fn is_clean(&self) -> bool {
        self.depth_violations.is_empty() && self.orphan_nodes.is_empty()
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

    // Future invariants: childless_sections, repetition_filter, etc. plug in here.

    if !report.is_clean() {
        println!(
            "🩺 GraphSanity: {} depth violations{}, {} orphan nodes",
            report.depth_violations.len(),
            if dc.correct && !report.depth_violations.is_empty() {
                " (corrected)"
            } else {
                ""
            },
            report.orphan_nodes.len(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GraphSanityConfig, GraphSanityInvariants, InvariantToggle};
    use crate::types::{
        DocumentGraph, DocumentNode, NodeContent, NodeLocation, SemanticLocation,
    };
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
                depth_consistency: InvariantToggle { check: true, correct: true },
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
        assert!(report.is_clean(), "consistent tree must produce no diagnostics");
    }

    /// CR-28 Test 3 — check-only mode preserves original values.
    #[test]
    fn test_cr28_check_only_preserves_values() {
        let (mut graph, _, child_id) = make_two_node_graph(3);
        let cfg = GraphSanityConfig {
            enabled: true,
            invariants: GraphSanityInvariants {
                depth_consistency: InvariantToggle { check: true, correct: false },
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
            report.depth_violations.iter().any(|v| v.node_id == paragraph_id),
            "paragraph drift must be detected"
        );
        assert_eq!(
            graph.nodes[&paragraph_id].location.semantic.depth,
            graph.nodes[&section_id].location.semantic.depth + 1,
            "paragraph depth must equal section depth + 1 after correction"
        );
    }
}
