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

use crate::config::{
    GraphSanityConfig, SectionHeightInvariantConfig, SectionOverlapCountInvariantConfig,
    SectionParagraphOverlapInvariantConfig, TopologyRebalanceConfig,
};
use crate::preprocessors::pdf::xhtml_parser::normalize_for_match;
use crate::types::{BoundingBox, DocumentGraph, DocumentNode, NodeId};
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

/// CR-68 — Per-node record of a Section/Paragraph overlap-demote violation.
#[derive(Debug, Clone)]
pub struct SectionOverlapViolation {
    pub node_id: NodeId,
    pub overlap_fraction: f32,
    pub threshold: f32,
    pub corrected: bool,
}

/// CR-69 — Per-node record of a section overlap-COUNT demote violation.
#[derive(Debug, Clone)]
pub struct SectionOverlapCountViolation {
    pub node_id: NodeId,
    pub overlap_count: u32,
    pub count_threshold: u32,
    pub corrected: bool,
}

/// CR-70 — Aggregate record of one topology-rebalance run. Unlike the per-node
/// demotion violations, the rebalance is a whole-tree rebuild, so the report
/// carries counts rather than one record per node.
#[derive(Debug, Clone, Default)]
pub struct TopologyRebalanceReport {
    /// Nodes whose `parent` pointer changed.
    pub reparented: usize,
    /// Nodes whose `location.semantic.depth` changed.
    pub depths_changed: usize,
    /// Demoted (non-Section) nodes that had children before the rebuild and
    /// are leaves after it (spurious levels collapsed).
    pub spurious_levels_collapsed: usize,
    /// Whether corrections were written back to the graph.
    pub corrected: bool,
}

/// Diagnostic output from a sanity-pipe run. Always populated when the pipe
/// runs; consumers decide what to do with it (log, attach to output, etc.).
#[derive(Debug, Default, Clone)]
pub struct SanityReport {
    pub depth_violations: Vec<DepthViolation>,
    pub orphan_nodes: Vec<NodeId>,
    pub section_height_violations: Vec<SectionHeightViolation>,
    pub section_overlap_violations: Vec<SectionOverlapViolation>,
    pub section_overlap_count_violations: Vec<SectionOverlapCountViolation>,
    /// CR-70 — present when the topology-rebalance step ran. `None` when the
    /// step is disabled in config.
    pub topology_rebalance: Option<TopologyRebalanceReport>,
}

impl SanityReport {
    pub fn is_clean(&self) -> bool {
        self.depth_violations.is_empty()
            && self.orphan_nodes.is_empty()
            && self.section_height_violations.is_empty()
            && self.section_overlap_violations.is_empty()
            && self.section_overlap_count_violations.is_empty()
            && self
                .topology_rebalance
                .as_ref()
                .map(|r| r.reparented == 0 && r.depths_changed == 0)
                .unwrap_or(true)
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

    let so = &config.invariants.section_paragraph_overlap;
    if so.check || so.correct {
        check_and_correct_section_overlap(graph, so, &mut report);
    }

    let soc = &config.invariants.section_overlap_count;
    if soc.check || soc.correct {
        check_and_correct_section_overlap_count(graph, soc, &mut report);
    }

    // CR-70 — topology rebalance runs LAST, after all node_type demotions have
    // settled. It rebuilds parent/child/depth from the surviving node set.
    let tr = &config.invariants.topology_rebalance;
    if tr.check || tr.correct {
        rebalance_topology(graph, tr, &mut report);
    }

    // Future invariants: childless_sections, repetition_filter, etc. plug in here.

    if !report.is_clean() {
        let rebalance_summary = report
            .topology_rebalance
            .as_ref()
            .map(|r| {
                format!(
                    ", topology rebalance: {} re-parented, {} depths changed, {} spurious levels collapsed{}",
                    r.reparented,
                    r.depths_changed,
                    r.spurious_levels_collapsed,
                    if r.corrected { " (applied)" } else { "" },
                )
            })
            .unwrap_or_default();
        println!(
            "🩺 GraphSanity: {} depth violations{}, {} orphan nodes, {} section-height violations{}, {} section-overlap violations{}, {} section-overlap-count violations{}{}",
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
            report.section_overlap_violations.len(),
            if so.correct && !report.section_overlap_violations.is_empty() {
                " (demoted to Paragraph)"
            } else {
                ""
            },
            report.section_overlap_count_violations.len(),
            if soc.correct && !report.section_overlap_count_violations.is_empty() {
                " (demoted to Paragraph)"
            } else {
                ""
            },
            rebalance_summary,
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

/// CR-68 — Section demoted when its bbox 2D-area-overlaps a same-page
/// Paragraph by more than `threshold` (fraction of the Section's own area).
///
/// Figure callouts misclassified as Sections sit inside figure regions that
/// also hold Paragraph content, so they overlap heavily; real section headers
/// occupy their own visual space (~0% overlap). Composes after CR-65's
/// height-bound: height removes the big-font callouts, overlap catches the
/// residual at-body-size ones. Demote is node_type-only (topology preserved,
/// so depth_consistency still holds).
///
/// `threshold == 0.0` is the OFF sentinel — early-return, no cost.
/// `bookmark_bypass`: never demote a Section whose normalized text matches a
/// bookmark-outline title (reuses CR-67's normalize_for_match). Precision
/// helper, NOT the safety mechanism (half the corpus has no outline).
fn check_and_correct_section_overlap(
    graph: &mut DocumentGraph,
    cfg: &SectionParagraphOverlapInvariantConfig,
    report: &mut SanityReport,
) {
    if cfg.threshold <= 0.0 {
        return; // OFF sentinel
    }

    // Same-page Paragraph bboxes (page, bbox).
    let paragraphs: Vec<(u32, BoundingBox)> = graph
        .nodes
        .values()
        .filter(|n| n.node_type == "Paragraph")
        .filter_map(|n| {
            n.location
                .physical
                .as_ref()
                .map(|p| (p.page, p.bounding_box.clone()))
        })
        .collect();

    // Normalized bookmark titles for the bypass (only if enabled + present).
    let bookmark_titles: Option<HashSet<String>> = if cfg.bookmark_bypass {
        graph.document_info.bookmark_data.as_ref().map(|bd| {
            bd.sections
                .iter()
                .map(|s| normalize_for_match(&s.title))
                .collect()
        })
    } else {
        None
    };

    // Pass 1 — find violators (immutable borrow).
    let mut violators: Vec<(NodeId, f32)> = Vec::new();
    for (id, n) in graph.nodes.iter() {
        if n.node_type != "Section" {
            continue;
        }
        let phys = match n.location.physical.as_ref() {
            Some(p) => p,
            None => continue,
        };
        let s = &phys.bounding_box;
        let section_area = s.width * s.height;
        if section_area <= 0.0 {
            continue;
        }
        // Bookmark-bypass: protect titles that match the outline.
        if let Some(titles) = &bookmark_titles {
            if titles.contains(&normalize_for_match(&n.content.text)) {
                continue;
            }
        }
        // Max overlap fraction vs same-page paragraphs.
        let max_overlap = paragraphs
            .iter()
            .filter(|(page, _)| *page == phys.page)
            .map(|(_, p)| {
                let ix = (s.x + s.width).min(p.x + p.width) - s.x.max(p.x);
                let iy = (s.y + s.height).min(p.y + p.height) - s.y.max(p.y);
                if ix <= 0.0 || iy <= 0.0 {
                    0.0
                } else {
                    (ix * iy) / section_area
                }
            })
            .fold(0.0_f32, f32::max);
        if max_overlap > cfg.threshold {
            violators.push((*id, max_overlap));
        }
    }

    // Pass 2 — record + (optionally) demote.
    for (node_id, overlap_fraction) in violators {
        report
            .section_overlap_violations
            .push(SectionOverlapViolation {
                node_id,
                overlap_fraction,
                threshold: cfg.threshold,
                corrected: cfg.correct,
            });
        if cfg.correct {
            if let Some(node) = graph.nodes.get_mut(&node_id) {
                node.node_type = "Paragraph".to_string();
            }
        }
    }
}

/// CR-69 — geometry-only overlap-COUNT demote (supersedes CR-68's fraction
/// rule). Demote a Section whose bbox overlaps at least `count_threshold`
/// same-page nodes of ANY type (excluding itself) by more than
/// `min_overlap_frac` of the Section's own area.
///
/// A figure callout misclassified as a Section sits in a cluttered figure
/// region and overlaps several sibling callouts + figure elements (count >= 3);
/// a real header — even one engulfed by a single over-merged Paragraph —
/// overlaps <= 2. So unlike CR-68's overlap-*fraction* (which can't tell a
/// figure callout from an over-merge victim, both high-fraction), the *count*
/// separates them on geometry alone — no bookmark-bypass needed. Composes after
/// CR-65 (height) + CR-68 (fraction, now off). node_type-only demote (topology
/// preserved, so depth_consistency still holds).
fn check_and_correct_section_overlap_count(
    graph: &mut DocumentGraph,
    cfg: &SectionOverlapCountInvariantConfig,
    report: &mut SanityReport,
) {
    if cfg.count_threshold == 0 {
        return; // a 0 threshold would demote every section — treat as OFF
    }

    // Every node with a physical bbox: (id, page, bbox). Any node type counts
    // toward a section's overlap tally — CR-69 is a figure-CLUSTER detector.
    let nodes: Vec<(NodeId, u32, BoundingBox)> = graph
        .nodes
        .iter()
        .filter_map(|(id, n)| {
            n.location
                .physical
                .as_ref()
                .map(|p| (*id, p.page, p.bounding_box.clone()))
        })
        .collect();

    // Pass 1 — find violators (immutable borrow).
    let mut violators: Vec<(NodeId, u32)> = Vec::new();
    for (id, n) in graph.nodes.iter() {
        if n.node_type != "Section" {
            continue;
        }
        let phys = match n.location.physical.as_ref() {
            Some(p) => p,
            None => continue,
        };
        let s = &phys.bounding_box;
        let section_area = s.width * s.height;
        if section_area <= 0.0 {
            continue;
        }
        let count = nodes
            .iter()
            .filter(|(other_id, page, _)| *other_id != *id && *page == phys.page)
            .filter(|(_, _, b)| {
                let ix = (s.x + s.width).min(b.x + b.width) - s.x.max(b.x);
                let iy = (s.y + s.height).min(b.y + b.height) - s.y.max(b.y);
                ix > 0.0 && iy > 0.0 && (ix * iy) / section_area > cfg.min_overlap_frac
            })
            .count() as u32;
        if count >= cfg.count_threshold {
            violators.push((*id, count));
        }
    }

    // Pass 2 — record + (optionally) demote.
    for (node_id, overlap_count) in violators {
        report
            .section_overlap_count_violations
            .push(SectionOverlapCountViolation {
                node_id,
                overlap_count,
                count_threshold: cfg.count_threshold,
                corrected: cfg.correct,
            });
        if cfg.correct {
            if let Some(node) = graph.nodes.get_mut(&node_id) {
                node.node_type = "Paragraph".to_string();
            }
        }
    }
}

// ─── CR-70 — Topology rebalance ───────────────────────────────────────────────

/// Reduce a font-family name to its typeface stem so weight/style variants of
/// the same face collapse together. Ported from the validated prototype
/// (`scripts/sb_rebalance_experiment.py::font_stem`).
///
/// `'ABCDEF+XCharter-BoldItalic'` → `'xcharter'`; `'TimesNewRomanPSMT'` →
/// `'timesnewroman'`; `'DejaVuSans'` → `'dejavusans'`.
///
/// Strategy: drop the subset prefix (`ABCDEF+`), take everything before the
/// first hyphen (the hyphen reliably separates the face from its weight, incl.
/// abbreviations like `-Regu`/`-Medi`), then peel any glued foundry/weight tags
/// (`PSMT`, `Bold`, `Italic`, …) repeatedly. Returns `"?"` for an empty/missing
/// family.
fn font_stem(fam: Option<&str>) -> String {
    let fam = match fam {
        Some(f) if !f.is_empty() => f,
        _ => return "?".to_string(),
    };
    // Drop subset prefix `ABCDEF+`, then take everything before the first hyphen.
    let after_plus = fam.rsplit('+').next().unwrap_or(fam);
    let before_hyphen = after_plus.split('-').next().unwrap_or(after_plus);
    // Peel glued weight/style/foundry suffixes (case-insensitive), repeatedly.
    const GLUED_SUFFIXES: &[&str] = &[
        "psmt", "bold", "italic", "oblique", "regular", "medium", "light", "semibold", "black",
        "ps", "mt",
    ];
    let mut s = before_hyphen.to_string();
    loop {
        let lower = s.to_lowercase();
        let mut peeled = false;
        for suf in GLUED_SUFFIXES {
            if lower.ends_with(suf) && lower.len() > suf.len() {
                s.truncate(s.len() - suf.len());
                peeled = true;
                break;
            }
        }
        if !peeled {
            break;
        }
    }
    let out = s.to_lowercase();
    if out.is_empty() {
        fam.to_lowercase()
    } else {
        out
    }
}

/// Numbering depth from a leading section-number prefix: `"3."` → 1, `"3.1."` →
/// 2, `"3.1.1."` → 3. Ported from the prototype's `numbering_level` / `NUM_RE`
/// (`^\s*\**\s*(\d+(?:\.\d+)*)`): count the dot-components of the matched number
/// and add one. Returns `None` when the text does not start with a number.
fn numbering_level(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    let mut i = 0;
    // Optional leading whitespace.
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    // Optional run of `*` (markdown emphasis markers).
    while i < bytes.len() && bytes[i] == b'*' {
        i += 1;
    }
    // Optional whitespace after the `*` run.
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    // Must start with a digit.
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return None;
    }
    // Capture `\d+(?:\.\d+)*` and count the dot-components.
    let mut dots = 0u32;
    let mut last_was_digit = false;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            i += 1;
            last_was_digit = true;
        } else if bytes[i] == b'.' && last_was_digit {
            // Lookahead: a `.` only extends the number if followed by a digit.
            if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                dots += 1;
                i += 1;
                last_was_digit = false;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    Some(dots + 1)
}

/// A surviving Section's level signal: numbering depth if present, else the
/// calibrated font-size-rank fallback. Mirrors the prototype's
/// `make_level_of(source="numbering")` (`num or fb`).
struct LevelInputs {
    numbering: HashMap<NodeId, Option<u32>>,
    size_fallback: HashMap<NodeId, u32>,
}

impl LevelInputs {
    fn level_of(&self, id: &NodeId) -> u32 {
        match self.numbering.get(id).copied().flatten() {
            Some(n) => n,
            None => self.size_fallback.get(id).copied().unwrap_or(1),
        }
    }
}

/// Lightweight view of a node used by the rebalance pass — extracted once so
/// the stack replay can run over an immutable snapshot before mutating the
/// graph. Mirrors the fields the prototype's `collect` pulls.
struct RebalanceNode {
    id: NodeId,
    is_open_section: bool,
}

/// Level for UNNUMBERED sections, calibrated against the numbered ones. Ported
/// from the prototype's `compute_size_fallback`.
///
/// The numbered sections (reliable levels) define a size→level map within the
/// main (plurality-stem) font family; an unnumbered section's level is
/// `1 + (count of distinct numbered main-family sizes strictly larger than it)`.
/// A heading larger than any numbered size → level 1; one matching the level-2
/// numbered size → level 2. Falls back to ranking ALL main-family section sizes
/// when the doc has no numbered sections.
///
/// `sections` is `(id, stem, size, is_numbered)` in any order (the plurality and
/// size-set computations are order-stable on insertion for tie-breaking).
fn compute_size_fallback(sections: &[(NodeId, String, Option<f32>, bool)]) -> HashMap<NodeId, u32> {
    // Plurality stem among sections (ties broken by first-seen order, matching
    // Python's `max(dict, key=dict.get)` on an insertion-ordered dict).
    let mut stem_counts: Vec<(String, usize)> = Vec::new();
    for (_, stem, _, _) in sections {
        if let Some(entry) = stem_counts.iter_mut().find(|(s, _)| s == stem) {
            entry.1 += 1;
        } else {
            stem_counts.push((stem.clone(), 1));
        }
    }
    let main = stem_counts
        .iter()
        .max_by_key(|(_, c)| *c)
        .map(|(s, _)| s.clone())
        .unwrap_or_else(|| "?".to_string());

    // Distinct numbered main-family sizes, descending.
    let mut numbered_sizes: Vec<f32> = collect_distinct_sizes(sections, &main, true);
    if numbered_sizes.is_empty() {
        // Wholly-unnumbered doc — rank all main-family section sizes.
        numbered_sizes = collect_distinct_sizes(sections, &main, false);
    }

    let mut fb = HashMap::new();
    for (id, _, size, _) in sections {
        let level = match size {
            None => (numbered_sizes.len() as u32).max(1),
            Some(sz) => 1 + numbered_sizes.iter().filter(|ns| **ns > *sz).count() as u32,
        };
        fb.insert(*id, level);
    }
    fb
}

/// Distinct font sizes among main-family sections, descending. When
/// `numbered_only`, restrict to the numbered ones. Uses the f32 bit pattern as
/// the dedup key (sizes come straight off the wire — exact equality, no
/// tolerance, matching the Python `set` semantics).
fn collect_distinct_sizes(
    sections: &[(NodeId, String, Option<f32>, bool)],
    main: &str,
    numbered_only: bool,
) -> Vec<f32> {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut sizes: Vec<f32> = Vec::new();
    for (_, stem, size, is_numbered) in sections {
        if stem != main {
            continue;
        }
        if numbered_only && !*is_numbered {
            continue;
        }
        if let Some(sz) = size {
            if seen.insert(sz.to_bits()) {
                sizes.push(*sz);
            }
        }
    }
    sizes.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    sizes
}

/// Per-doc offset that aligns numbering to the outline's convention: the
/// bookmark level of the first numbered section minus its numbering depth.
/// Ported from the prototype's `first_number_offset`. RFC outlines insert the
/// title as level-1 (offset +1); academic outlines are aligned (offset 0).
///
/// In the live rebalance the depth is derived from stack POSITION, so the
/// offset is implicit in the front-matter that pushes onto the stack before the
/// first numbered section — this helper is retained for parity with the
/// prototype's scoring path and for callers that want the explicit offset.
#[allow(dead_code)]
fn first_number_offset(
    ordered_sections: &[NodeId],
    matched: &HashMap<NodeId, u32>,
    numbering: &HashMap<NodeId, Option<u32>>,
) -> i64 {
    for id in ordered_sections {
        if let (Some(&bm), Some(&Some(num))) = (matched.get(id), numbering.get(id)) {
            return bm as i64 - num as i64;
        }
    }
    0
}

/// CR-70 — rebuild parent/child/depth topology over the surviving nodes.
///
/// Replays the stack-based outline build (the same algorithm
/// `builder.rs::find_parent` uses, ported from the validated prototype's
/// `rebalance`) over the post-demotion node set in `text_order`:
///
/// - A surviving Section opens a level: compute its level `L` (numbering depth,
///   else calibrated font-size rank), pop the stack while `stack_top.L >= L`,
///   set `depth = min(stack_position, max_section_depth)`, parent = stack top,
///   then push.
/// - Every other node (content, or a node demoted to a non-Section type) is a
///   leaf attached to the current open section at
///   `depth = min(section_depth + 1, max_total_depth)`.
///
/// Then rewrite each node's `parent`, `children` (ordered by `text_order`), and
/// `location.semantic.depth`. The Document root stays at depth 0 and parents the
/// top-level nodes. `check`-only mode records the report but writes nothing.
fn rebalance_topology(
    graph: &mut DocumentGraph,
    cfg: &TopologyRebalanceConfig,
    report: &mut SanityReport,
) {
    let root_id = graph.document_info.root_id;

    // ── Snapshot the surviving non-Document nodes in text_order. ──
    let mut ordered: Vec<(u32, NodeId)> = graph
        .nodes
        .iter()
        .filter(|(id, n)| **id != root_id && n.node_type != "Document")
        .map(|(id, n)| (n.text_order.unwrap_or(u32::MAX), *id))
        .collect();
    // Stable sort by text_order; ties keep a deterministic order by id.
    ordered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let order: Vec<NodeId> = ordered.into_iter().map(|(_, id)| id).collect();

    // ── Per-section level inputs (numbering + calibrated size fallback). ──
    let mut numbering: HashMap<NodeId, Option<u32>> = HashMap::new();
    let mut section_fields: Vec<(NodeId, String, Option<f32>, bool)> = Vec::new();
    for id in &order {
        let n = &graph.nodes[id];
        if n.node_type != "Section" {
            continue;
        }
        let num = numbering_level(&n.content.text);
        numbering.insert(*id, num);
        let stem = font_stem(n.style_info.as_ref().and_then(|s| s.font_family.as_deref()));
        let size = n.style_info.as_ref().and_then(|s| s.font_size);
        section_fields.push((*id, stem, size, num.is_some()));
    }
    let size_fallback = compute_size_fallback(&section_fields);
    let levels = LevelInputs {
        numbering,
        size_fallback,
    };

    // Snapshot which nodes open a level (surviving Sections).
    let snapshot: Vec<RebalanceNode> = order
        .iter()
        .map(|id| RebalanceNode {
            id: *id,
            is_open_section: graph.nodes[id].node_type == "Section",
        })
        .collect();

    // ── Stack replay: depth from position, capped; parent = nearest ancestor
    //    one level shallower. ──
    //
    // Depth derivation matches the validated prototype exactly
    // (`d = min(len(stack), section_cap)`). The prototype scores depths only; the
    // parent pointer is this port's addition. We parent a section to the topmost
    // stack frame strictly shallower than its assigned depth (rather than the
    // bare stack top) so that capping keeps `parent.depth + 1 == child.depth`:
    // when the cap forces several deeply-nested sections to the same depth, they
    // become siblings under the nearest ancestor at `cap - 1` (the CR's
    // "level-cap siblings"), not a degenerate same-depth child chain.
    struct Frame {
        level: u32,
        depth: u32,
        id: NodeId,
    }
    let mut stack: Vec<Frame> = vec![Frame {
        level: 0,
        depth: 0,
        id: root_id,
    }];
    let mut new_parent: HashMap<NodeId, NodeId> = HashMap::new();
    let mut new_depth: HashMap<NodeId, u32> = HashMap::new();

    for node in &snapshot {
        if node.is_open_section {
            let l = levels.level_of(&node.id);
            while stack.len() > 1 && stack.last().unwrap().level >= l {
                stack.pop();
            }
            // depth = stack position (len before push), capped at max_section_depth.
            let depth = (stack.len() as u32).min(cfg.max_section_depth);
            // Parent = topmost frame strictly shallower than `depth` (== the
            // bare stack top in the uncapped case; the ancestor at `depth - 1`
            // when the cap collapsed adjacent frames to equal depth).
            let parent = stack
                .iter()
                .rev()
                .find(|f| f.depth < depth)
                .map(|f| f.id)
                .unwrap_or(root_id);
            new_parent.insert(node.id, parent);
            new_depth.insert(node.id, depth);
            stack.push(Frame {
                level: l,
                depth,
                id: node.id,
            });
        } else {
            let top = stack.last().unwrap();
            let depth = (top.depth + 1).min(cfg.max_total_depth);
            new_parent.insert(node.id, top.id);
            new_depth.insert(node.id, depth);
        }
    }

    // ── Diagnose: count re-parents, depth changes, and collapsed spurious
    //    levels (non-Section nodes that had children and become leaves). ──
    let mut rebalance_report = TopologyRebalanceReport {
        corrected: cfg.correct,
        ..Default::default()
    };
    for id in &order {
        let n = &graph.nodes[id];
        if new_parent.get(id).copied() != n.parent {
            rebalance_report.reparented += 1;
        }
        if new_depth.get(id).copied().unwrap_or(n.location.semantic.depth)
            != n.location.semantic.depth
        {
            rebalance_report.depths_changed += 1;
        }
        if n.node_type != "Section" && !n.children.is_empty() {
            rebalance_report.spurious_levels_collapsed += 1;
        }
    }

    if !cfg.correct {
        report.topology_rebalance = Some(rebalance_report);
        return;
    }

    // ── Apply: rewrite parent + depth, then rebuild children (text_order). ──
    for id in &order {
        if let Some(node) = graph.nodes.get_mut(id) {
            node.parent = new_parent.get(id).copied();
            if let Some(&d) = new_depth.get(id) {
                node.location.semantic.depth = d;
            }
        }
    }

    // Children: ordered by text_order via `order` (already text_order-sorted).
    let mut children: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for id in &order {
        if let Some(parent) = new_parent.get(id) {
            children.entry(*parent).or_default().push(*id);
        }
    }
    // Reset every node's children, then assign the rebuilt lists. The root is
    // included so its top-level children are refreshed too.
    let all_ids: Vec<NodeId> = graph.nodes.keys().copied().collect();
    for id in all_ids {
        let new_children = children.remove(&id).unwrap_or_default();
        if let Some(node) = graph.nodes.get_mut(&id) {
            node.children = new_children;
        }
    }

    report.topology_rebalance = Some(rebalance_report);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        GraphSanityConfig, GraphSanityInvariants, InvariantToggle, SectionHeightInvariantConfig,
        SectionOverlapCountInvariantConfig, SectionParagraphOverlapInvariantConfig,
        TopologyRebalanceConfig,
    };
    use crate::types::{
        BookmarkData, BookmarkSection, BoundingBox, DocumentGraph, DocumentNode, PhysicalLocation,
        StyleMetadata,
    };
    use uuid::Uuid;

    /// CR-70 is default-ON, so the pre-CR-70 (CR-28/65/68/69) tests — written
    /// to exercise one demotion/depth invariant in isolation — must explicitly
    /// disable the rebalance, which would otherwise rewrite depth/parent and
    /// change those tests' (pre-rebalance) expectations. The rebalance itself
    /// is covered by its own CR-70 tests below.
    fn topology_off() -> TopologyRebalanceConfig {
        TopologyRebalanceConfig {
            check: false,
            correct: false,
            max_section_depth: 3,
            max_total_depth: 4,
        }
    }

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
                topology_rebalance: topology_off(),
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
                topology_rebalance: topology_off(),
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
                topology_rebalance: topology_off(),
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

    // ──────────────────────────────────────────────────────────────────────
    // CR-68 — Section/Paragraph overlap-demote invariant tests
    // ──────────────────────────────────────────────────────────────────────

    /// Build a graph: root → (section Section, paragraph Paragraph). The caller
    /// controls each node's (page, x, y, width, height) and the section's text.
    /// Both sit at depth 1; the section is the title candidate (its own height,
    /// so CR-65 no-ops on it). Returns (graph, root_id, section_id, paragraph_id).
    #[allow(clippy::too_many_arguments)]
    fn make_section_paragraph_graph(
        section_text: &str,
        sec: (u32, f32, f32, f32, f32),
        para: (u32, f32, f32, f32, f32),
    ) -> (DocumentGraph, NodeId, NodeId, NodeId) {
        let root_id = Uuid::new_v4();
        let section_id = Uuid::new_v4();
        let paragraph_id = Uuid::new_v4();
        let mut graph = DocumentGraph::new_with_root(root_id);

        let mut root = DocumentNode::new_with_id(root_id, "Document", "root".into());
        root.children.push(section_id);
        root.children.push(paragraph_id);
        root.location.semantic.depth = 0;
        graph.nodes.insert(root_id, root);

        let bbox = |b: (u32, f32, f32, f32, f32)| PhysicalLocation {
            page: b.0,
            bounding_box: BoundingBox {
                x: b.1,
                y: b.2,
                width: b.3,
                height: b.4,
            },
        };

        let mut section = DocumentNode::new_with_id(section_id, "Section", section_text.into());
        section.parent = Some(root_id);
        section.location.semantic.depth = 1;
        section.text_order = Some(0);
        section.location.physical = Some(bbox(sec));
        graph.nodes.insert(section_id, section);

        let mut paragraph = DocumentNode::new_with_id(paragraph_id, "Paragraph", "body".into());
        paragraph.parent = Some(root_id);
        paragraph.location.semantic.depth = 1;
        paragraph.text_order = Some(1);
        paragraph.location.physical = Some(bbox(para));
        graph.nodes.insert(paragraph_id, paragraph);

        (graph, root_id, section_id, paragraph_id)
    }

    /// Config that enables the CR-68 overlap invariant at the given threshold,
    /// leaving the other invariants at their defaults.
    fn overlap_config(threshold: f32, bookmark_bypass: bool) -> GraphSanityConfig {
        GraphSanityConfig {
            enabled: true,
            invariants: GraphSanityInvariants {
                section_paragraph_overlap: SectionParagraphOverlapInvariantConfig {
                    check: true,
                    correct: true,
                    threshold,
                    bookmark_bypass,
                },
                topology_rebalance: topology_off(),
                ..Default::default()
            },
        }
    }

    /// CR-68 Test 1 — Section overlapping a same-page Paragraph by > 0.20 of the
    /// section's own area, no bookmark data → demoted to Paragraph.
    /// Section bbox (0,0,100,100) area=10000; paragraph (0,0,100,50) overlaps
    /// 100×50=5000 → 0.50 > 0.20 → demote.
    #[test]
    fn test_cr68_demotes_section_overlapping_paragraph() {
        let (mut graph, _, section_id, _) = make_section_paragraph_graph(
            "Figure callout",
            (1, 0.0, 0.0, 100.0, 100.0),
            (1, 0.0, 0.0, 100.0, 50.0),
        );
        let report = apply(&mut graph, &overlap_config(0.20, true));
        assert_eq!(report.section_overlap_violations.len(), 1);
        assert_eq!(report.section_overlap_violations[0].node_id, section_id);
        assert!(report.section_overlap_violations[0].corrected);
        assert_eq!(graph.nodes[&section_id].node_type, "Paragraph");
    }

    /// CR-68 Test 2 — same overlap geometry, but bookmark_data contains a title
    /// matching the section's text and bookmark_bypass is on → kept as Section.
    #[test]
    fn test_cr68_bookmark_bypass_protects_matching_section() {
        let (mut graph, _, section_id, _) = make_section_paragraph_graph(
            "3.1. Approach 1",
            (1, 0.0, 0.0, 100.0, 100.0),
            (1, 0.0, 0.0, 100.0, 50.0),
        );
        // Outline writes it as "3.1 Approach 1"; normalize_for_match aligns them.
        graph.document_info.bookmark_data = Some(BookmarkData {
            sections: vec![BookmarkSection {
                title: "3.1 Approach 1".into(),
                order: 0,
                level: 1,
            }],
        });
        let report = apply(&mut graph, &overlap_config(0.20, true));
        assert!(
            report.section_overlap_violations.is_empty(),
            "bookmark-matching section must be protected"
        );
        assert_eq!(graph.nodes[&section_id].node_type, "Section");
    }

    /// CR-68 Test 3 — overlap < 0.20 → kept as Section.
    /// Paragraph (0,0,100,10) overlaps section (0,0,100,100) by 1000/10000=0.10.
    #[test]
    fn test_cr68_keeps_section_below_threshold() {
        let (mut graph, _, section_id, _) = make_section_paragraph_graph(
            "Real header",
            (1, 0.0, 0.0, 100.0, 100.0),
            (1, 0.0, 0.0, 100.0, 10.0),
        );
        let report = apply(&mut graph, &overlap_config(0.20, true));
        assert!(
            report.section_overlap_violations.is_empty(),
            "overlap below threshold must not flag"
        );
        assert_eq!(graph.nodes[&section_id].node_type, "Section");
    }

    /// CR-68 Test 4 — threshold 0.0 is the OFF sentinel: early-return, no
    /// violations, node unchanged even with overlapping geometry.
    #[test]
    fn test_cr68_threshold_zero_is_off_sentinel() {
        let (mut graph, _, section_id, _) = make_section_paragraph_graph(
            "Figure callout",
            (1, 0.0, 0.0, 100.0, 100.0),
            (1, 0.0, 0.0, 100.0, 50.0),
        );
        let report = apply(&mut graph, &overlap_config(0.0, true));
        assert!(
            report.section_overlap_violations.is_empty(),
            "threshold 0.0 must early-return (OFF sentinel)"
        );
        assert_eq!(graph.nodes[&section_id].node_type, "Section");
    }

    /// CR-68 Test 5 — a real section sitting directly above a paragraph (boxes
    /// touch but do not overlap) → kept as Section. The RFC tight-packing case.
    /// Section (0,0,100,20) occupies y∈[0,20]; paragraph (0,20,100,80) occupies
    /// y∈[20,100] → iy = 20 - 20 = 0 → 0% overlap.
    #[test]
    fn test_cr68_keeps_real_section_adjacent_to_paragraph() {
        let (mut graph, _, section_id, _) = make_section_paragraph_graph(
            "1. Introduction",
            (1, 0.0, 0.0, 100.0, 20.0),
            (1, 0.0, 20.0, 100.0, 80.0),
        );
        let report = apply(&mut graph, &overlap_config(0.20, true));
        assert!(
            report.section_overlap_violations.is_empty(),
            "adjacent (touching, non-overlapping) section must be kept"
        );
        assert_eq!(graph.nodes[&section_id].node_type, "Section");
    }

    // ──────────────────────────────────────────────────────────────────────
    // CR-69 — geometry-only overlap-COUNT demote invariant tests
    // ──────────────────────────────────────────────────────────────────────

    /// Build root → 1 candidate Section (at `sec`) + N other nodes (each
    /// `(node_type, page, x, y, w, h)`). Returns (graph, section_id).
    fn make_overlap_count_graph(
        sec: (u32, f32, f32, f32, f32),
        others: &[(&str, u32, f32, f32, f32, f32)],
    ) -> (DocumentGraph, NodeId) {
        let root_id = Uuid::new_v4();
        let section_id = Uuid::new_v4();
        let mut graph = DocumentGraph::new_with_root(root_id);

        let mut root = DocumentNode::new_with_id(root_id, "Document", "root".into());
        root.location.semantic.depth = 0;
        root.children.push(section_id);

        let bbox = |p: u32, x: f32, y: f32, w: f32, h: f32| PhysicalLocation {
            page: p,
            bounding_box: BoundingBox { x, y, width: w, height: h },
        };

        let mut section = DocumentNode::new_with_id(section_id, "Section", "candidate".into());
        section.parent = Some(root_id);
        section.location.semantic.depth = 1;
        section.location.physical = Some(bbox(sec.0, sec.1, sec.2, sec.3, sec.4));
        graph.nodes.insert(section_id, section);

        for (i, o) in others.iter().enumerate() {
            let id = Uuid::new_v4();
            root.children.push(id);
            let mut node = DocumentNode::new_with_id(id, o.0, format!("other{i}"));
            node.parent = Some(root_id);
            node.location.semantic.depth = 1;
            node.location.physical = Some(bbox(o.1, o.2, o.3, o.4, o.5));
            graph.nodes.insert(id, node);
        }
        graph.nodes.insert(root_id, root);
        (graph, section_id)
    }

    fn overlap_count_config(count_threshold: u32, min_overlap_frac: f32) -> GraphSanityConfig {
        GraphSanityConfig {
            enabled: true,
            invariants: GraphSanityInvariants {
                section_overlap_count: SectionOverlapCountInvariantConfig {
                    check: true,
                    correct: true,
                    count_threshold,
                    min_overlap_frac,
                },
                topology_rebalance: topology_off(),
                ..Default::default()
            },
        }
    }

    /// CR-69 Test 1 — a Section overlapping 3 same-page nodes of mixed type
    /// (the figure-cluster case) → demoted to Paragraph at count_threshold 3.
    #[test]
    fn test_cr69_demotes_section_in_cluster() {
        let (mut graph, section_id) = make_overlap_count_graph(
            (1, 0.0, 0.0, 100.0, 100.0),
            &[
                ("Paragraph", 1, 0.0, 0.0, 50.0, 50.0),
                ("Figure", 1, 50.0, 0.0, 50.0, 50.0),
                ("Section", 1, 0.0, 50.0, 50.0, 50.0),
            ],
        );
        let report = apply(&mut graph, &overlap_count_config(3, 0.0));
        let v: Vec<_> = report
            .section_overlap_count_violations
            .iter()
            .filter(|v| v.node_id == section_id)
            .collect();
        assert_eq!(v.len(), 1, "clustered section must be flagged once");
        assert_eq!(v[0].overlap_count, 3);
        assert_eq!(graph.nodes[&section_id].node_type, "Paragraph");
    }

    /// CR-69 Test 2 — only 2 overlapping nodes (e.g. an over-merged-paragraph
    /// victim) → count 2 < 3 → kept as Section.
    #[test]
    fn test_cr69_keeps_section_with_two_overlaps() {
        let (mut graph, section_id) = make_overlap_count_graph(
            (1, 0.0, 0.0, 100.0, 100.0),
            &[
                ("Paragraph", 1, 0.0, 0.0, 100.0, 100.0),
                ("Paragraph", 1, 0.0, 0.0, 30.0, 30.0),
            ],
        );
        let report = apply(&mut graph, &overlap_count_config(3, 0.0));
        assert!(
            report.section_overlap_count_violations.is_empty(),
            "two overlaps is below the count threshold"
        );
        assert_eq!(graph.nodes[&section_id].node_type, "Section");
    }

    /// CR-69 Test 3 — an isolated real header overlapping nothing (others sit on
    /// another page) → kept as Section.
    #[test]
    fn test_cr69_keeps_isolated_section() {
        let (mut graph, section_id) = make_overlap_count_graph(
            (1, 0.0, 0.0, 100.0, 20.0),
            &[
                ("Paragraph", 2, 0.0, 0.0, 100.0, 100.0),
                ("Paragraph", 2, 0.0, 0.0, 100.0, 100.0),
                ("Paragraph", 2, 0.0, 0.0, 100.0, 100.0),
            ],
        );
        let report = apply(&mut graph, &overlap_count_config(3, 0.0));
        assert!(report.section_overlap_count_violations.is_empty());
        assert_eq!(graph.nodes[&section_id].node_type, "Section");
    }

    /// CR-69 Test 4 — min_overlap_frac filters trivial touches: a Section
    /// overlapping 3 nodes, but each by < min_overlap_frac of its own area →
    /// not counted → kept. Section area = 10000; each node overlaps by 100
    /// (1%), below the 0.10 floor.
    #[test]
    fn test_cr69_min_frac_filters_trivial_touches() {
        let (mut graph, section_id) = make_overlap_count_graph(
            (1, 0.0, 0.0, 100.0, 100.0),
            &[
                ("Paragraph", 1, 90.0, 90.0, 10.0, 10.0),
                ("Paragraph", 1, 0.0, 90.0, 10.0, 10.0),
                ("Paragraph", 1, 90.0, 0.0, 10.0, 10.0),
            ],
        );
        let report = apply(&mut graph, &overlap_count_config(3, 0.10));
        assert!(
            report.section_overlap_count_violations.is_empty(),
            "sub-threshold overlaps must not count"
        );
        assert_eq!(graph.nodes[&section_id].node_type, "Section");
    }

    /// CR-69 Test 5 — count_threshold 0 is an OFF guard: early-return, nothing
    /// demoted even with a heavy cluster.
    #[test]
    fn test_cr69_count_threshold_zero_is_off() {
        let (mut graph, section_id) = make_overlap_count_graph(
            (1, 0.0, 0.0, 100.0, 100.0),
            &[
                ("Paragraph", 1, 0.0, 0.0, 50.0, 50.0),
                ("Figure", 1, 50.0, 0.0, 50.0, 50.0),
                ("Section", 1, 0.0, 50.0, 50.0, 50.0),
            ],
        );
        let report = apply(&mut graph, &overlap_count_config(0, 0.0));
        assert!(
            report.section_overlap_count_violations.is_empty(),
            "count_threshold 0 must early-return (OFF guard)"
        );
        assert_eq!(graph.nodes[&section_id].node_type, "Section");
    }

    // ──────────────────────────────────────────────────────────────────────
    // CR-70 — topology-rebalance tests
    // ──────────────────────────────────────────────────────────────────────

    /// Spec for one node fed to `make_rebalance_graph`:
    /// `(node_type, text, font_family, font_size, initial_depth, initial_parent_idx)`.
    /// `initial_parent_idx` is an index into the node list (0-based, in the
    /// order given); `None` parents to root. The initial depth/parent/children
    /// may be deliberately stale — that is what the rebalance rebuilds.
    struct NodeSpec {
        node_type: &'static str,
        text: &'static str,
        font_family: Option<&'static str>,
        font_size: Option<f32>,
        depth: u32,
        parent_idx: Option<usize>,
    }

    /// Build a graph from an ordered list of `NodeSpec`s. `text_order` is the
    /// list position; initial `parent`/`children`/`depth` come from each spec
    /// so we can simulate post-demotion stale topology. Returns the graph plus
    /// the node ids in list order.
    fn make_rebalance_graph(specs: &[NodeSpec]) -> (DocumentGraph, NodeId, Vec<NodeId>) {
        let root_id = Uuid::new_v4();
        let mut graph = DocumentGraph::new_with_root(root_id);
        let ids: Vec<NodeId> = specs.iter().map(|_| Uuid::new_v4()).collect();

        let mut root = DocumentNode::new_with_id(root_id, "Document", "root".into());
        root.location.semantic.depth = 0;

        for (i, spec) in specs.iter().enumerate() {
            let id = ids[i];
            let mut node = DocumentNode::new_with_id(id, spec.node_type, spec.text.into());
            node.text_order = Some(i as u32);
            node.location.semantic.depth = spec.depth;
            match spec.parent_idx {
                Some(p) => {
                    node.parent = Some(ids[p]);
                }
                None => {
                    node.parent = Some(root_id);
                    root.children.push(id);
                }
            }
            if spec.font_family.is_some() || spec.font_size.is_some() {
                node.style_info = Some(StyleMetadata {
                    font_class: String::new(),
                    font_size: spec.font_size,
                    is_bold: false,
                    is_italic: false,
                    font_family: spec.font_family.map(|s| s.to_string()),
                    foreground_color: None,
                    background_color: None,
                });
            }
            graph.nodes.insert(id, node);
        }
        // Wire up children for non-root parents from the parent_idx links.
        for (i, spec) in specs.iter().enumerate() {
            if let Some(p) = spec.parent_idx {
                let child = ids[i];
                if let Some(parent) = graph.nodes.get_mut(&ids[p]) {
                    parent.children.push(child);
                }
            }
        }
        graph.nodes.insert(root_id, root);
        (graph, root_id, ids)
    }

    /// Config enabling ONLY the rebalance (every demotion off) at the given
    /// caps. Isolates the topology rebuild from the node_type demotions.
    fn rebalance_only_config(max_section_depth: u32, max_total_depth: u32) -> GraphSanityConfig {
        GraphSanityConfig {
            enabled: true,
            invariants: GraphSanityInvariants {
                depth_consistency: InvariantToggle {
                    check: false,
                    correct: false,
                },
                section_height_bounded_by_title: SectionHeightInvariantConfig {
                    check: false,
                    correct: false,
                    tolerance: 2.0,
                },
                section_paragraph_overlap: SectionParagraphOverlapInvariantConfig {
                    check: false,
                    correct: false,
                    threshold: 0.0,
                    bookmark_bypass: false,
                },
                section_overlap_count: SectionOverlapCountInvariantConfig {
                    check: false,
                    correct: false,
                    count_threshold: 3,
                    min_overlap_frac: 0.0,
                },
                topology_rebalance: TopologyRebalanceConfig {
                    check: true,
                    correct: true,
                    max_section_depth,
                    max_total_depth,
                },
            },
        }
    }

    /// CR-70 Test 1 — numbered-hierarchy rebuild. Sections "1.", "1.1", "2."
    /// (with stale depths) rebuild to depths 1, 2, 1 with correct parents.
    #[test]
    fn test_cr70_numbered_hierarchy_rebuild() {
        let specs = [
            NodeSpec { node_type: "Section", text: "1. Introduction", font_family: None, font_size: None, depth: 9, parent_idx: None },
            NodeSpec { node_type: "Section", text: "1.1 Background", font_family: None, font_size: None, depth: 9, parent_idx: None },
            NodeSpec { node_type: "Section", text: "2. Methods", font_family: None, font_size: None, depth: 9, parent_idx: None },
        ];
        let (mut graph, root_id, ids) = make_rebalance_graph(&specs);
        apply(&mut graph, &rebalance_only_config(3, 4));

        assert_eq!(graph.nodes[&ids[0]].location.semantic.depth, 1, "1. Introduction → depth 1");
        assert_eq!(graph.nodes[&ids[0]].parent, Some(root_id));
        assert_eq!(graph.nodes[&ids[1]].location.semantic.depth, 2, "1.1 Background → depth 2");
        assert_eq!(graph.nodes[&ids[1]].parent, Some(ids[0]), "1.1 parented to 1.");
        assert_eq!(graph.nodes[&ids[2]].location.semantic.depth, 1, "2. Methods → depth 1");
        assert_eq!(graph.nodes[&ids[2]].parent, Some(root_id));
        // Children rebuilt: 1. has 1.1 as its only child; 2. is a leaf.
        assert_eq!(graph.nodes[&ids[0]].children, vec![ids[1]]);
        assert!(graph.nodes[&ids[2]].children.is_empty());
    }

    /// CR-70 Test 2 — gap-collapse. Numbering levels 1 then 3 (a skipped level)
    /// collapse to consecutive stack-position depths 1, 2.
    #[test]
    fn test_cr70_gap_collapse() {
        let specs = [
            NodeSpec { node_type: "Section", text: "1. Top", font_family: None, font_size: None, depth: 1, parent_idx: None },
            NodeSpec { node_type: "Section", text: "1.1.1 Deep", font_family: None, font_size: None, depth: 3, parent_idx: None },
        ];
        let (mut graph, _root, ids) = make_rebalance_graph(&specs);
        apply(&mut graph, &rebalance_only_config(3, 4));

        assert_eq!(graph.nodes[&ids[0]].location.semantic.depth, 1);
        assert_eq!(
            graph.nodes[&ids[1]].location.semantic.depth, 2,
            "numbering level 3 collapses to stack-position depth 2 (no gap)"
        );
        assert_eq!(graph.nodes[&ids[1]].parent, Some(ids[0]));
    }

    /// CR-70 Test 3 — cap. A 1/1.1/1.1.1/1.1.1.1 nest with max_section_depth 3
    /// clamps the level-4 section to depth 3 as a sibling under the level-2
    /// section, and content under it sits at the total cap (depth 4).
    #[test]
    fn test_cr70_cap_flattens_deep_nest() {
        let specs = [
            NodeSpec { node_type: "Section", text: "1. A", font_family: None, font_size: None, depth: 1, parent_idx: None },
            NodeSpec { node_type: "Section", text: "1.1 B", font_family: None, font_size: None, depth: 2, parent_idx: None },
            NodeSpec { node_type: "Section", text: "1.1.1 C", font_family: None, font_size: None, depth: 3, parent_idx: None },
            NodeSpec { node_type: "Section", text: "1.1.1.1 D", font_family: None, font_size: None, depth: 4, parent_idx: None },
            NodeSpec { node_type: "Paragraph", text: "body under D", font_family: None, font_size: None, depth: 5, parent_idx: None },
        ];
        let (mut graph, _root, ids) = make_rebalance_graph(&specs);
        apply(&mut graph, &rebalance_only_config(3, 4));

        assert_eq!(graph.nodes[&ids[0]].location.semantic.depth, 1);
        assert_eq!(graph.nodes[&ids[1]].location.semantic.depth, 2);
        assert_eq!(graph.nodes[&ids[2]].location.semantic.depth, 3);
        assert_eq!(
            graph.nodes[&ids[3]].location.semantic.depth, 3,
            "level-4 section clamped to max_section_depth (3)"
        );
        assert_eq!(
            graph.nodes[&ids[3]].parent, Some(ids[1]),
            "capped section is a level-3 sibling under the level-2 ancestor"
        );
        assert_eq!(
            graph.nodes[&ids[4]].location.semantic.depth, 4,
            "content under a cap-level section sits at the total cap (4)"
        );
        assert_eq!(graph.nodes[&ids[4]].parent, Some(ids[3]));
        // No depth exceeds the total cap.
        assert!(graph.nodes.values().all(|n| n.location.semantic.depth <= 4));
    }

    /// CR-70 Test 4 — a demoted Section (now a Paragraph with stale children)
    /// re-parents its children to the enclosing section and becomes a leaf.
    #[test]
    fn test_cr70_demoted_section_reparents_children_and_becomes_leaf() {
        // node 1 was a Section, demoted to Paragraph upstream; it still carries
        // node 2 as a stale child at the spurious extra depth.
        let specs = [
            NodeSpec { node_type: "Section", text: "1. Introduction", font_family: None, font_size: None, depth: 1, parent_idx: None },
            NodeSpec { node_type: "Paragraph", text: "Figure 2 (demoted)", font_family: None, font_size: None, depth: 2, parent_idx: Some(0) },
            NodeSpec { node_type: "Paragraph", text: "figure body text", font_family: None, font_size: None, depth: 3, parent_idx: Some(1) },
        ];
        let (mut graph, _root, ids) = make_rebalance_graph(&specs);
        // Precondition: the demoted node has a child (the stale spurious level).
        assert!(!graph.nodes[&ids[1]].children.is_empty());

        let report = apply(&mut graph, &rebalance_only_config(3, 4));

        // The demoted node is now a leaf...
        assert!(
            graph.nodes[&ids[1]].children.is_empty(),
            "demoted Paragraph must become a leaf"
        );
        // ...and its former child re-attaches to the enclosing section.
        assert_eq!(
            graph.nodes[&ids[2]].parent, Some(ids[0]),
            "orphaned child re-parents to the enclosing section"
        );
        assert_eq!(graph.nodes[&ids[1]].parent, Some(ids[0]));
        assert_eq!(graph.nodes[&ids[1]].location.semantic.depth, 2);
        assert_eq!(graph.nodes[&ids[2]].location.semantic.depth, 2);
        // The collapse is reported.
        let tr = report.topology_rebalance.unwrap();
        assert!(tr.spurious_levels_collapsed >= 1);
    }

    /// CR-70 Test 5 — font-size-rank fallback for an unnumbered section.
    /// Numbered sections (12pt L1, 10pt L2) calibrate the size→level map; an
    /// unnumbered "Appendix" at 14pt (larger than any numbered size) → level 1.
    #[test]
    fn test_cr70_font_size_rank_fallback_unnumbered() {
        let specs = [
            NodeSpec { node_type: "Section", text: "1. Introduction", font_family: Some("ABCDEF+XCharter-Bold"), font_size: Some(12.0), depth: 1, parent_idx: None },
            NodeSpec { node_type: "Section", text: "1.1 Background", font_family: Some("ABCDEF+XCharter-Bold"), font_size: Some(10.0), depth: 2, parent_idx: None },
            NodeSpec { node_type: "Section", text: "Appendix", font_family: Some("ABCDEF+XCharter-Bold"), font_size: Some(14.0), depth: 9, parent_idx: None },
        ];
        let (mut graph, root_id, ids) = make_rebalance_graph(&specs);
        apply(&mut graph, &rebalance_only_config(3, 4));

        assert_eq!(graph.nodes[&ids[0]].location.semantic.depth, 1);
        assert_eq!(graph.nodes[&ids[1]].location.semantic.depth, 2);
        assert_eq!(
            graph.nodes[&ids[2]].location.semantic.depth, 1,
            "unnumbered Appendix at 14pt (> all numbered sizes) → level 1"
        );
        assert_eq!(graph.nodes[&ids[2]].parent, Some(root_id));
    }

    /// CR-70 Test 6 — idempotency. A second rebalance over an already-rebalanced
    /// graph re-parents nothing and changes no depth.
    #[test]
    fn test_cr70_idempotent_second_run_is_noop() {
        let specs = [
            NodeSpec { node_type: "Section", text: "1. Introduction", font_family: None, font_size: None, depth: 9, parent_idx: None },
            NodeSpec { node_type: "Section", text: "1.1 Background", font_family: None, font_size: None, depth: 9, parent_idx: None },
            NodeSpec { node_type: "Paragraph", text: "body", font_family: None, font_size: None, depth: 9, parent_idx: None },
            NodeSpec { node_type: "Section", text: "2. Methods", font_family: None, font_size: None, depth: 9, parent_idx: None },
        ];
        let (mut graph, _root, _ids) = make_rebalance_graph(&specs);
        apply(&mut graph, &rebalance_only_config(3, 4));

        // Second run: no topology changes.
        let report = apply(&mut graph, &rebalance_only_config(3, 4));
        {
            let tr = report
                .topology_rebalance
                .as_ref()
                .expect("rebalance report present");
            assert_eq!(tr.reparented, 0, "second run must re-parent nothing");
            assert_eq!(tr.depths_changed, 0, "second run must change no depth");
            assert_eq!(tr.spurious_levels_collapsed, 0, "no spurious levels remain");
        }
        assert!(
            report.is_clean(),
            "idempotent second run leaves the report clean"
        );
    }

    /// CR-70 Test 7 — check-only mode records the report but writes nothing.
    #[test]
    fn test_cr70_check_only_does_not_mutate() {
        let specs = [
            NodeSpec { node_type: "Section", text: "1. Introduction", font_family: None, font_size: None, depth: 9, parent_idx: None },
            NodeSpec { node_type: "Section", text: "1.1 Background", font_family: None, font_size: None, depth: 9, parent_idx: None },
        ];
        let (mut graph, _root, ids) = make_rebalance_graph(&specs);
        let cfg = GraphSanityConfig {
            enabled: true,
            invariants: GraphSanityInvariants {
                depth_consistency: InvariantToggle { check: false, correct: false },
                topology_rebalance: TopologyRebalanceConfig {
                    check: true,
                    correct: false,
                    max_section_depth: 3,
                    max_total_depth: 4,
                },
                ..Default::default()
            },
        };
        let report = apply(&mut graph, &cfg);
        let tr = report.topology_rebalance.expect("report present in check mode");
        assert!(!tr.corrected, "check-only must report corrected = false");
        assert!(tr.depths_changed >= 1, "check-only still diagnoses pending changes");
        // The stale depth (9) is preserved because nothing was written.
        assert_eq!(graph.nodes[&ids[1]].location.semantic.depth, 9);
    }

    // ── CR-70 helper unit tests (new numbering / font-stem logic) ──

    #[test]
    fn test_cr70_numbering_level() {
        assert_eq!(numbering_level("3. Title"), Some(1));
        assert_eq!(numbering_level("3.1. Sub"), Some(2));
        assert_eq!(numbering_level("3.1.1 Deep"), Some(3));
        assert_eq!(numbering_level("  **2.4** bolded"), Some(2));
        assert_eq!(numbering_level("Abstract"), None);
        assert_eq!(numbering_level("Appendix A"), None);
        // A trailing dot not followed by a digit does not extend the number.
        assert_eq!(numbering_level("4. Results"), Some(1));
    }

    #[test]
    fn test_cr70_font_stem() {
        assert_eq!(font_stem(Some("ABCDEF+XCharter-BoldItalic")), "xcharter");
        assert_eq!(font_stem(Some("TimesNewRomanPSMT")), "timesnewroman");
        assert_eq!(font_stem(Some("TimesNewRomanPS-BoldMT")), "timesnewroman");
        assert_eq!(font_stem(Some("NimbusRomNo9L-Regu")), "nimbusromno9l");
        assert_eq!(font_stem(Some("DejaVuSans")), "dejavusans");
        assert_eq!(font_stem(None), "?");
        assert_eq!(font_stem(Some("")), "?");
    }
}
