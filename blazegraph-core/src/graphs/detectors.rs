//! CR-71A — Evidence-first section detectors (flag-only).
//!
//! The evidence-first redesign (CR-71) reshapes section demotion from "each
//! invariant independently mutates `node_type`" into: detectors **flag** (never
//! mutate), evidence aggregates to **document-level verdicts**, and a single
//! prune step (see `prune.rs`) owns all conversion decisions.
//!
//! This module holds:
//! - the transient [`SectionEvidence`] sidecar (and [`NodeFlags`]) threaded
//!   through the mini-pipeline's signatures — never on `DocumentNode`, never on
//!   the wire;
//! - the CR-65 / CR-68 / CR-69 geometric predicates, factored into shared
//!   helpers so the parked-off old mutating fns and the new flaggers compute
//!   identical geometry;
//! - the flag-only detectors, which read the geometry helpers and write
//!   [`NodeFlags`] into the sidecar (they take `&DocumentGraph`, read-only);
//! - [`SectionEvidence::aggregate_verdicts`], which computes the document-level
//!   `main_font` / `bad_fonts` verdict (ported from the validated prototype's
//!   `run_badfont_sweep`).
//!
//! See `docs/P2/core/change-requests/CR-71-font-family-figure-sweep-evidence-first.md`.

use crate::config::{
    SectionHeightInvariantConfig, SectionOverlapCountInvariantConfig,
    SectionParagraphOverlapInvariantConfig,
};
use crate::types::{BoundingBox, DocumentGraph, DocumentNode, NodeId};
use std::collections::{HashMap, HashSet};

use super::graph_sanity::font_stem;

/// Per-node flag record produced by the detectors. Records which of the three
/// geometric predicates fired plus the node's font stem (so the verdict
/// aggregation can group by family). One entry per *flagged* Section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFlags {
    /// The section's font-family stem (reduced via `font_stem`).
    pub font_stem: String,
    /// CR-65 predicate fired (section bbox.height exceeds title × tolerance).
    pub height_flag: bool,
    /// CR-68 predicate fired (section overlaps a same-page Paragraph by more
    /// than the configured fraction of its own area).
    pub overlap_flag: bool,
    /// CR-69 predicate fired (section overlaps >= count_threshold same-page
    /// nodes of any type).
    pub count_flag: bool,
}

impl NodeFlags {
    fn new(font_stem: String) -> Self {
        Self {
            font_stem,
            height_flag: false,
            overlap_flag: false,
            count_flag: false,
        }
    }

    /// Any geometric predicate fired.
    pub fn any(&self) -> bool {
        self.height_flag || self.overlap_flag || self.count_flag
    }
}

/// Transient sidecar carrying detector evidence through the sanity mini-pipeline.
///
/// Constructed in `graph_sanity::apply()`, threaded into the detectors and the
/// prune step, and dropped at the end of `apply()`. **Not** on `DocumentNode`
/// (which derives `Serialize` — byte-in/byte-out forbids transient analysis
/// state on the wire) and **not** on `SanityReport` (which stays pure
/// diagnostics-out).
#[derive(Debug, Default, Clone)]
pub struct SectionEvidence {
    /// One entry per Section that at least one detector flagged. Keyed by node
    /// id. (Only flagged sections are stored — an unflagged section has no
    /// record, matching the "flagged set" the artifact dumps.)
    pub per_node: HashMap<NodeId, NodeFlags>,
    /// Plurality font stem among detected Sections. Filled by
    /// `aggregate_verdicts`.
    pub main_font: Option<String>,
    /// Font stems with >= 1 geometric flag AND != main_font. Filled by
    /// `aggregate_verdicts`. The main font is never a member (catastrophe
    /// insurance: one clustered real heading can't condemn the document).
    pub bad_fonts: HashSet<String>,
}

impl SectionEvidence {
    /// Ensure a `NodeFlags` entry exists for `id` and return a mutable handle.
    /// The font stem is recorded on first insert.
    fn entry_mut(&mut self, id: NodeId, stem: &str) -> &mut NodeFlags {
        self.per_node
            .entry(id)
            .or_insert_with(|| NodeFlags::new(stem.to_string()))
    }

    /// CR-71A — aggregate per-node flags into the document-level verdict.
    ///
    /// Ported from the prototype's `run_badfont_sweep`:
    /// - `main_font` = **plurality font stem among detected Sections** (all
    ///   Sections in the graph, not just flagged ones — robust to docs whose
    ///   headings are a different family than the body). Ties broken by
    ///   first-seen order in a stable Section scan (mirrors Python's
    ///   `max(dict, key=dict.get)` on an insertion-ordered dict).
    /// - `bad_fonts` = stems with **>= 1 geometric flag AND stem != main_font**.
    ///   The main/plurality font is never taggable as bad.
    pub fn aggregate_verdicts(&mut self, graph: &DocumentGraph) {
        // Plurality stem among ALL detected Sections (insertion-ordered tally
        // for a deterministic tie-break). Use text_order to make the scan
        // order stable regardless of HashMap iteration order.
        let mut sections: Vec<&DocumentNode> = graph
            .nodes
            .values()
            .filter(|n| n.node_type == "Section")
            .collect();
        sections.sort_by_key(|n| n.text_order.unwrap_or(u32::MAX));

        let mut stem_counts: Vec<(String, usize)> = Vec::new();
        for n in &sections {
            let stem = font_stem(n.style_info.as_ref().and_then(|s| s.font_family.as_deref()));
            if let Some(entry) = stem_counts.iter_mut().find(|(s, _)| *s == stem) {
                entry.1 += 1;
            } else {
                stem_counts.push((stem, 1));
            }
        }
        // First stem achieving the max count wins ties — matches the Python
        // reference `max(dict, key=dict.get)` (first-max). `max_by_key` would
        // return the LAST max, which on a tie picks the wrong font. stem_counts
        // is already in text_order, so "first" = earliest-appearing section font.
        let main = stem_counts
            .iter()
            .fold(None::<&(String, usize)>, |best, cur| match best {
                Some(b) if b.1 >= cur.1 => Some(b),
                _ => Some(cur),
            })
            .map(|(s, _)| s.clone());
        self.main_font = main.clone();

        // bad_fonts = flagged stems minus the main font.
        let mut bad: HashSet<String> = HashSet::new();
        for flags in self.per_node.values() {
            if !flags.any() {
                continue;
            }
            if Some(&flags.font_stem) == main.as_ref() {
                continue; // main font is never taggable as bad
            }
            bad.insert(flags.font_stem.clone());
        }
        self.bad_fonts = bad;
    }
}

// ─── Shared geometric predicates (CR-65 / CR-68 / CR-69) ─────────────────────
//
// These are the read-only geometry the parked-off old mutating fns and the new
// flaggers both rely on. Factored here so the two paths can never drift.

/// CR-65 — Document title = first depth-1 Section in source order with a
/// physical bounding box. Returns its bbox.height, or `None` if no such node
/// exists (e.g. MD channel, short doc with no depth-1 sections).
pub fn find_title_height(graph: &DocumentGraph) -> Option<f32> {
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

/// CR-68 — fraction (of the section's own area) by which it overlaps the
/// most-overlapping same-page Paragraph. `paragraphs` is `(page, bbox)`.
/// Returns 0.0 when the section has no positive overlap or zero area.
pub fn max_paragraph_overlap_fraction(
    section_page: u32,
    section_bbox: &BoundingBox,
    paragraphs: &[(u32, BoundingBox)],
) -> f32 {
    let section_area = section_bbox.width * section_bbox.height;
    if section_area <= 0.0 {
        return 0.0;
    }
    paragraphs
        .iter()
        .filter(|(page, _)| *page == section_page)
        .map(|(_, p)| bbox_overlap_area(section_bbox, p) / section_area)
        .fold(0.0_f32, f32::max)
}

/// CR-69 — count of same-page nodes (of ANY type, excluding the section itself)
/// whose overlap with the section exceeds `min_overlap_frac` of the section's
/// own area. `nodes` is `(id, page, bbox)`.
pub fn same_page_overlap_count(
    section_id: NodeId,
    section_page: u32,
    section_bbox: &BoundingBox,
    nodes: &[(NodeId, u32, BoundingBox)],
    min_overlap_frac: f32,
) -> u32 {
    let section_area = section_bbox.width * section_bbox.height;
    if section_area <= 0.0 {
        return 0;
    }
    nodes
        .iter()
        .filter(|(other_id, page, _)| *other_id != section_id && *page == section_page)
        .filter(|(_, _, b)| bbox_overlap_area(section_bbox, b) / section_area > min_overlap_frac)
        .count() as u32
}

/// 2D intersection area of two bounding boxes (0.0 when they don't overlap).
fn bbox_overlap_area(a: &BoundingBox, b: &BoundingBox) -> f32 {
    let ix = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let iy = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    if ix <= 0.0 || iy <= 0.0 {
        0.0
    } else {
        ix * iy
    }
}

// ─── Flag-only detectors ─────────────────────────────────────────────────────
//
// Each detector takes `(&DocumentGraph, &mut SectionEvidence, &cfg)` — read-only
// on the graph. They reuse the shared predicates above and set one boolean per
// flagged Section. They never read or write `node_type`.

/// CR-65 detector — flag Sections whose bbox.height exceeds the document
/// title's height × tolerance. No-op when there is no title-bearing depth-1
/// Section.
pub fn flag_section_height(
    graph: &DocumentGraph,
    evidence: &mut SectionEvidence,
    cfg: &SectionHeightInvariantConfig,
) {
    let title_height = match find_title_height(graph) {
        Some(h) => h,
        None => return,
    };
    let threshold = title_height * cfg.tolerance;

    let flagged: Vec<(NodeId, String)> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.node_type == "Section")
        .filter_map(|(id, n)| {
            n.location
                .physical
                .as_ref()
                .filter(|p| p.bounding_box.height > threshold)
                .map(|_| (*id, stem_of(n)))
        })
        .collect();

    for (id, stem) in flagged {
        evidence.entry_mut(id, &stem).height_flag = true;
    }
}

/// CR-68 detector — flag Sections that overlap a same-page Paragraph by more
/// than `cfg.threshold` of the section's own area. `threshold <= 0.0` is the
/// OFF sentinel (early-return, no cost). Bookmark-bypass (CR-67) is preserved as
/// a precision helper.
pub fn flag_section_overlap(
    graph: &DocumentGraph,
    evidence: &mut SectionEvidence,
    cfg: &SectionParagraphOverlapInvariantConfig,
) {
    if cfg.threshold <= 0.0 {
        return; // OFF sentinel
    }

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

    let bookmark_titles: Option<HashSet<String>> = if cfg.bookmark_bypass {
        graph.document_info.bookmark_data.as_ref().map(|bd| {
            bd.sections
                .iter()
                .map(|s| crate::preprocessors::pdf::xhtml_parser::normalize_for_match(&s.title))
                .collect()
        })
    } else {
        None
    };

    let mut flagged: Vec<(NodeId, String)> = Vec::new();
    for (id, n) in graph.nodes.iter() {
        if n.node_type != "Section" {
            continue;
        }
        let phys = match n.location.physical.as_ref() {
            Some(p) => p,
            None => continue,
        };
        if let Some(titles) = &bookmark_titles {
            if titles.contains(
                &crate::preprocessors::pdf::xhtml_parser::normalize_for_match(&n.content.text),
            ) {
                continue;
            }
        }
        let frac = max_paragraph_overlap_fraction(phys.page, &phys.bounding_box, &paragraphs);
        if frac > cfg.threshold {
            flagged.push((*id, stem_of(n)));
        }
    }

    for (id, stem) in flagged {
        evidence.entry_mut(id, &stem).overlap_flag = true;
    }
}

/// CR-69 detector — flag Sections whose bbox overlaps >= `cfg.count_threshold`
/// same-page nodes of any type (the figure-cluster geometry seed).
/// `count_threshold == 0` is treated as OFF (a 0 threshold would flag every
/// section).
pub fn flag_section_overlap_count(
    graph: &DocumentGraph,
    evidence: &mut SectionEvidence,
    cfg: &SectionOverlapCountInvariantConfig,
) {
    if cfg.count_threshold == 0 {
        return; // OFF
    }

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

    let mut flagged: Vec<(NodeId, String)> = Vec::new();
    for (id, n) in graph.nodes.iter() {
        if n.node_type != "Section" {
            continue;
        }
        let phys = match n.location.physical.as_ref() {
            Some(p) => p,
            None => continue,
        };
        let count = same_page_overlap_count(
            *id,
            phys.page,
            &phys.bounding_box,
            &nodes,
            cfg.min_overlap_frac,
        );
        if count >= cfg.count_threshold {
            flagged.push((*id, stem_of(n)));
        }
    }

    for (id, stem) in flagged {
        evidence.entry_mut(id, &stem).count_flag = true;
    }
}

/// A node's font-family stem (reduced via `font_stem`).
fn stem_of(n: &DocumentNode) -> String {
    font_stem(n.style_info.as_ref().and_then(|s| s.font_family.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        SectionHeightInvariantConfig, SectionOverlapCountInvariantConfig,
        SectionParagraphOverlapInvariantConfig,
    };
    use crate::types::{BoundingBox, DocumentGraph, DocumentNode, PhysicalLocation, StyleMetadata};
    use uuid::Uuid;

    /// Spec for one node in a detector fixture.
    struct Spec {
        node_type: &'static str,
        text: &'static str,
        font_family: Option<&'static str>,
        page: u32,
        bbox: (f32, f32, f32, f32), // x, y, w, h
    }

    fn style(font_family: Option<&str>) -> StyleMetadata {
        StyleMetadata {
            font_class: String::new(),
            font_size: None,
            is_bold: false,
            is_italic: false,
            font_family: font_family.map(|s| s.to_string()),
            foreground_color: None,
            background_color: None,
        }
    }

    /// Build root → N nodes from specs (all at depth 1). Returns (graph, ids).
    fn build(specs: &[Spec]) -> (DocumentGraph, Vec<NodeId>) {
        let root_id = Uuid::new_v4();
        let mut graph = DocumentGraph::new_with_root(root_id);
        let mut root = DocumentNode::new_with_id(root_id, "Document", "root".into());
        root.location.semantic.depth = 0;

        let mut ids = Vec::new();
        for (i, s) in specs.iter().enumerate() {
            let id = Uuid::new_v4();
            ids.push(id);
            root.children.push(id);
            let mut n = DocumentNode::new_with_id(id, s.node_type, s.text.into());
            n.parent = Some(root_id);
            n.text_order = Some(i as u32);
            n.location.semantic.depth = 1;
            n.location.physical = Some(PhysicalLocation {
                page: s.page,
                bounding_box: BoundingBox {
                    x: s.bbox.0,
                    y: s.bbox.1,
                    width: s.bbox.2,
                    height: s.bbox.3,
                },
            });
            n.style_info = Some(style(s.font_family));
            graph.nodes.insert(id, n);
        }
        graph.nodes.insert(root_id, root);
        (graph, ids)
    }

    fn assert_no_node_type_mutation(graph: &DocumentGraph, ids: &[NodeId], expected: &[&str]) {
        for (id, exp) in ids.iter().zip(expected.iter()) {
            assert_eq!(
                graph.nodes[id].node_type, *exp,
                "detector must not mutate node_type"
            );
        }
    }

    /// CR-71A — the height detector flags the tall section, writes height_flag,
    /// and never touches node_type.
    #[test]
    fn test_height_detector_flags_without_mutating() {
        // idx0 = title (h=18, depth-1 first-in-order), idx1 = tall section (h=50).
        let (graph, ids) = build(&[
            Spec {
                node_type: "Section",
                text: "Title",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 0.0, 100.0, 18.0),
            },
            Spec {
                node_type: "Section",
                text: "FLOPS",
                font_family: Some("DejaVuSans"),
                page: 1,
                bbox: (0.0, 30.0, 100.0, 50.0),
            },
        ]);
        let cfg = SectionHeightInvariantConfig {
            check: true,
            correct: true,
            tolerance: 2.0,
        };
        let mut evidence = SectionEvidence::default();
        flag_section_height(&graph, &mut evidence, &cfg);

        // Threshold = 18 * 2.0 = 36; title h=18 (not flagged), FLOPS h=50 > 36 (flagged).
        assert!(
            !evidence.per_node.contains_key(&ids[0]),
            "title not flagged"
        );
        let f = evidence
            .per_node
            .get(&ids[1])
            .expect("tall section flagged");
        assert!(f.height_flag);
        assert!(!f.overlap_flag && !f.count_flag);
        assert_eq!(f.font_stem, "dejavusans");
        assert_no_node_type_mutation(&graph, &ids, &["Section", "Section"]);
    }

    /// CR-71A — the count detector flags the clustered section (>=3 same-page
    /// overlaps), writes count_flag, never mutates node_type.
    #[test]
    fn test_count_detector_flags_without_mutating() {
        let (graph, ids) = build(&[
            Spec {
                node_type: "Section",
                text: "1B",
                font_family: Some("DejaVuSans"),
                page: 1,
                bbox: (0.0, 0.0, 100.0, 100.0),
            },
            Spec {
                node_type: "Paragraph",
                text: "p",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 0.0, 50.0, 50.0),
            },
            Spec {
                node_type: "Figure",
                text: "f",
                font_family: None,
                page: 1,
                bbox: (50.0, 0.0, 50.0, 50.0),
            },
            Spec {
                node_type: "Section",
                text: "x",
                font_family: Some("DejaVuSans"),
                page: 1,
                bbox: (0.0, 50.0, 50.0, 50.0),
            },
        ]);
        let cfg = SectionOverlapCountInvariantConfig {
            check: true,
            correct: true,
            count_threshold: 3,
            min_overlap_frac: 0.0,
        };
        let mut evidence = SectionEvidence::default();
        flag_section_overlap_count(&graph, &mut evidence, &cfg);

        let f = evidence
            .per_node
            .get(&ids[0])
            .expect("clustered section flagged");
        assert!(f.count_flag);
        assert_no_node_type_mutation(&graph, &ids, &["Section", "Paragraph", "Figure", "Section"]);
    }

    /// CR-71A — the overlap-fraction detector flags a section sitting on top of a
    /// same-page paragraph; OFF-sentinel (threshold 0) flags nothing.
    #[test]
    fn test_overlap_detector_and_off_sentinel() {
        let (graph, ids) = build(&[
            Spec {
                node_type: "Section",
                text: "callout",
                font_family: Some("DejaVuSans"),
                page: 1,
                bbox: (0.0, 0.0, 100.0, 100.0),
            },
            Spec {
                node_type: "Paragraph",
                text: "body",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 0.0, 100.0, 100.0),
            },
        ]);
        // Off-sentinel: nothing flagged.
        let off = SectionParagraphOverlapInvariantConfig {
            check: true,
            correct: true,
            threshold: 0.0,
            bookmark_bypass: false,
        };
        let mut ev_off = SectionEvidence::default();
        flag_section_overlap(&graph, &mut ev_off, &off);
        assert!(ev_off.per_node.is_empty(), "threshold 0 is OFF");

        // On: full overlap (frac 1.0 > 0.5) flags the section.
        let on = SectionParagraphOverlapInvariantConfig {
            check: true,
            correct: true,
            threshold: 0.5,
            bookmark_bypass: false,
        };
        let mut ev = SectionEvidence::default();
        flag_section_overlap(&graph, &mut ev, &on);
        assert!(ev.per_node.get(&ids[0]).unwrap().overlap_flag);
        assert!(!ev.per_node.contains_key(&ids[1]));
    }

    /// CR-71A — `aggregate_verdicts`: main_font = plurality section stem;
    /// bad_fonts = flagged stems minus main.
    #[test]
    fn test_aggregate_verdicts_plurality_and_bad() {
        // 3 Times sections (plurality), 2 DejaVuSans sections (the figure font).
        let (graph, ids) = build(&[
            Spec {
                node_type: "Section",
                text: "1. A",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 0.0, 10.0, 10.0),
            },
            Spec {
                node_type: "Section",
                text: "2. B",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 20.0, 10.0, 10.0),
            },
            Spec {
                node_type: "Section",
                text: "3. C",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 40.0, 10.0, 10.0),
            },
            Spec {
                node_type: "Section",
                text: "FLOPS",
                font_family: Some("DejaVuSans"),
                page: 1,
                bbox: (0.0, 60.0, 10.0, 10.0),
            },
            Spec {
                node_type: "Section",
                text: "1B",
                font_family: Some("DejaVuSans"),
                page: 1,
                bbox: (0.0, 80.0, 10.0, 10.0),
            },
        ]);
        let mut evidence = SectionEvidence::default();
        // Flag one DejaVuSans section (geometric seed) + one Times section.
        evidence.entry_mut(ids[3], "dejavusans").count_flag = true;
        evidence.entry_mut(ids[0], "times").height_flag = true;

        evidence.aggregate_verdicts(&graph);
        assert_eq!(evidence.main_font.as_deref(), Some("times"));
        // DejaVuSans flagged + != main → bad; Times flagged but == main → not bad.
        assert!(evidence.bad_fonts.contains("dejavusans"));
        assert!(!evidence.bad_fonts.contains("times"), "main font never bad");
        assert_eq!(evidence.bad_fonts.len(), 1);
    }

    /// CR-71A — main-font safety: even when a main-font section is the only one
    /// flagged, the main font is never condemned (catastrophe insurance).
    #[test]
    fn test_aggregate_verdicts_main_never_bad() {
        let (graph, ids) = build(&[
            Spec {
                node_type: "Section",
                text: "1. A",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 0.0, 10.0, 10.0),
            },
            Spec {
                node_type: "Section",
                text: "2. B",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 20.0, 10.0, 10.0),
            },
        ]);
        let mut evidence = SectionEvidence::default();
        evidence.entry_mut(ids[0], "times").count_flag = true; // a clustered real heading
        evidence.aggregate_verdicts(&graph);
        assert_eq!(evidence.main_font.as_deref(), Some("times"));
        assert!(
            evidence.bad_fonts.is_empty(),
            "one clustered main-font heading can't condemn the doc"
        );
    }

    /// CR-71A — multiple detectors firing on the same section accumulate flags in
    /// one `NodeFlags` record.
    #[test]
    fn test_multiple_flags_accumulate() {
        let (graph, ids) = build(&[
            Spec {
                node_type: "Section",
                text: "Title",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 0.0, 100.0, 18.0),
            },
            Spec {
                node_type: "Section",
                text: "FLOPS",
                font_family: Some("DejaVuSans"),
                page: 1,
                bbox: (0.0, 0.0, 100.0, 100.0),
            },
            Spec {
                node_type: "Paragraph",
                text: "p",
                font_family: Some("Times"),
                page: 1,
                bbox: (0.0, 0.0, 50.0, 50.0),
            },
            Spec {
                node_type: "Figure",
                text: "f",
                font_family: None,
                page: 1,
                bbox: (50.0, 0.0, 50.0, 50.0),
            },
            Spec {
                node_type: "Section",
                text: "x",
                font_family: Some("DejaVuSans"),
                page: 1,
                bbox: (0.0, 50.0, 50.0, 50.0),
            },
        ]);
        let mut evidence = SectionEvidence::default();
        flag_section_height(
            &graph,
            &mut evidence,
            &SectionHeightInvariantConfig {
                check: true,
                correct: true,
                tolerance: 2.0,
            },
        );
        flag_section_overlap_count(
            &graph,
            &mut evidence,
            &SectionOverlapCountInvariantConfig {
                check: true,
                correct: true,
                count_threshold: 3,
                min_overlap_frac: 0.0,
            },
        );
        let f = evidence.per_node.get(&ids[1]).expect("FLOPS flagged");
        assert!(
            f.height_flag && f.count_flag,
            "both predicates recorded on one node"
        );
    }
}
