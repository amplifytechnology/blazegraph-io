//! CR-71 — The single section-prune step (the only graph mutator before the
//! CR-70 topology rebalance).
//!
//! CR-71A landed the plumbing with a no-op body; **CR-71B v1** fills it with the
//! `geo + font >= 2` policy (see `prune_sections`): when `prune_on_detection` is
//! on, a flagged section whose font is in the document's `bad_fonts` verdict is
//! demoted to Paragraph; otherwise the step is observation-only. When
//! `emit_evidence_artifact` is set it also dumps the per-doc `<doc>.evidence.json`
//! handle that fed the prototype (`scripts/sb_cr71_prune_prototype.py`).
//!
//! The CR-70 rebalance, which runs next, rebuilds topology over the survivors.

use crate::config::SectionPruneConfig;
use crate::graphs::detectors::SectionEvidence;
use crate::types::{DocumentGraph, NodeId};
use serde::Serialize;
use std::collections::BTreeMap;

/// Summary of what the prune step observed, written into `SanityReport`. Pure
/// diagnostics-out (the working evidence is the transient sidecar). Counts only
/// — never one record per node — matching the CR-70 rebalance-report style.
#[derive(Debug, Clone, Default)]
pub struct SectionPruneSummary {
    /// Sections that at least one detector flagged.
    pub flagged: usize,
    /// Sections demoted by the prune body (geo + font >= 2). 0 when
    /// `prune_on_detection` is off (observation-only).
    pub pruned: usize,
    /// Document-level main-font verdict (plurality section font stem).
    pub main_font: Option<String>,
    /// Document-level bad-font verdict (flagged stems minus the main font).
    pub bad_fonts: Vec<String>,
    /// Whether the master mutate switch (`prune_on_detection`) was on. When
    /// false the step is flag + log + (debug) artifact only.
    pub prune_on_detection: bool,
}

/// CR-71 — the prune step. Slotted into `graph_sanity::apply()` between the
/// (parked-off) demote block and `rebalance_topology` — the only graph mutator
/// in the evidence-first path. The CR-70 rebalance (next step) rebuilds
/// parent/child/depth over whatever Sections survive.
///
/// CR-71B v1 policy — **geo + font >= 2**: a Section is demoted iff it carries a
/// geometric flag (it has a `per_node` entry — height/overlap/count fired) AND
/// its font stem is in the document's `bad_fonts` verdict (colored by the
/// confirmed-bad font cluster). The two confirmations — a direct geometric
/// signal plus bad-font-cluster membership — are what make the sweep safe: a
/// real header in an odd font (one signal) survives. Demotion is node_type-only.
///
/// Reach note (Sb6 prototype finding): with the current three geometric
/// detectors this fires only where geometry corroborates a bad font — alphafold's
/// figure callouts, corpus-wide. Broadening to non-geometric figure FPs
/// (attention/word2vec font-outliers) needs an added direct signal — deferred to
/// a follow-up CR (the "more detectors is a lever" finding). Gated by
/// `cfg.prune_on_detection`.
pub fn prune_sections(
    graph: &mut DocumentGraph,
    evidence: &SectionEvidence,
    report: &mut crate::graphs::graph_sanity::SanityReport,
    cfg: &SectionPruneConfig,
) {
    let mut bad_fonts: Vec<String> = evidence.bad_fonts.iter().cloned().collect();
    bad_fonts.sort();
    let flagged = evidence.per_node.values().filter(|f| f.any()).count();

    // Debug artifact reflects the observed flagged set (emitted pre-demotion —
    // node_type isn't part of it). Default off; never part of bgraph.
    if cfg.emit_evidence_artifact {
        if let Err(e) = emit_evidence_artifact(graph, evidence) {
            eprintln!("⚠️  CR-71: failed to write evidence artifact: {e}");
        }
    }

    // geo + font >= 2: per_node holds only flagged sections (>= 1 geometric
    // flag), so the gate reduces to "this flagged section's font is confirmed
    // bad". node_type-only demote; rebalance_topology rebuilds the tree.
    let mut pruned = 0;
    if cfg.prune_on_detection {
        let to_demote: Vec<NodeId> = evidence
            .per_node
            .iter()
            .filter(|(_, f)| f.any() && evidence.bad_fonts.contains(&f.font_stem))
            .map(|(id, _)| *id)
            .collect();
        for id in to_demote {
            if let Some(n) = graph.nodes.get_mut(&id) {
                if n.node_type == "Section" {
                    n.node_type = "Paragraph".to_string();
                    pruned += 1;
                }
            }
        }
    }

    report.section_prune = Some(SectionPruneSummary {
        flagged,
        pruned,
        main_font: evidence.main_font.clone(),
        bad_fonts,
        prune_on_detection: cfg.prune_on_detection,
    });
}

/// One flagged section's row in the evidence artifact — a lean **handle** into
/// the output graph JSON, not a copy of it. Keyed by `node_id` (+ `text_order`
/// as a secondary join key). The section's text / font / bbox live in the graph
/// JSON the Python prototype already loads, so they are NOT duplicated here —
/// keeping the pipeline output the single source of truth for section features.
#[derive(Debug, Serialize)]
struct EvidenceSection {
    node_id: String,
    text_order: Option<u32>,
    height_flag: bool,
    overlap_flag: bool,
    count_flag: bool,
}

/// Top-level shape of `<doc>.evidence.json`.
#[derive(Debug, Serialize)]
struct EvidenceArtifact {
    main_font: Option<String>,
    bad_fonts: Vec<String>,
    sections: Vec<EvidenceSection>,
}

/// CR-71A — write the per-doc `<doc>.evidence.json` debug artifact.
///
/// PATH CONVENTION (a flagged fork — see the CR-71A report): `graph_sanity::apply`
/// has access to neither the storage handle nor the cache dir / pdf hash (and
/// the CR forbids a `processor.rs` change to thread them in). So the artifact is
/// derived from what the graph itself carries: the cache root is taken from
/// `BLAZEGRAPH_CACHE_DIR` (the same env var the CLI / sb_eval.sh use), defaulting
/// to `cache`, and the filename stem from `document_info.parse_provenance
/// .source_filename`. Result: `{cache}/evidence/<source-stem>.evidence.json`,
/// a separate file landing next to the `cache/` tree the Python prototype reads.
/// When no provenance is present (legacy/MD graphs), the source hash or a fixed
/// `unknown` stem is used. Never part of bgraph.
fn emit_evidence_artifact(
    graph: &DocumentGraph,
    evidence: &SectionEvidence,
) -> std::io::Result<()> {
    // Stable, deterministic ordering of flagged sections (by text_order).
    let mut flagged: Vec<(NodeId, u32)> = evidence
        .per_node
        .iter()
        .filter(|(_, f)| f.any())
        .map(|(id, _)| {
            (
                *id,
                graph
                    .nodes
                    .get(id)
                    .and_then(|n| n.text_order)
                    .unwrap_or(u32::MAX),
            )
        })
        .collect();
    flagged.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    let mut sections = Vec::with_capacity(flagged.len());
    for (id, _) in flagged {
        let n = match graph.nodes.get(&id) {
            Some(n) => n,
            None => continue,
        };
        let flags = &evidence.per_node[&id];
        sections.push(EvidenceSection {
            node_id: id.to_string(),
            text_order: n.text_order,
            height_flag: flags.height_flag,
            overlap_flag: flags.overlap_flag,
            count_flag: flags.count_flag,
        });
    }

    let mut bad_fonts: Vec<String> = evidence.bad_fonts.iter().cloned().collect();
    bad_fonts.sort();
    let artifact = EvidenceArtifact {
        main_font: evidence.main_font.clone(),
        bad_fonts,
        sections,
    };

    let cache_root = std::env::var("BLAZEGRAPH_CACHE_DIR").unwrap_or_else(|_| "cache".to_string());
    let dir = format!("{cache_root}/evidence");
    std::fs::create_dir_all(&dir)?;
    let stem = doc_stem(graph);
    let path = format!("{dir}/{stem}.evidence.json");
    let json = serde_json::to_string_pretty(&artifact).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    println!("🧾 CR-71A: evidence artifact → {path}");
    Ok(())
}

/// File stem for the evidence artifact, derived from provenance. Prefers the
/// source filename (basename without extension), then the source hash, then a
/// fixed `unknown`. Sanitized to a filesystem-safe token.
fn doc_stem(graph: &DocumentGraph) -> String {
    let prov = graph.document_info.parse_provenance.as_ref();
    let raw = prov
        .map(|p| {
            // Strip directory + a single trailing extension from the filename.
            let base = p
                .source_filename
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&p.source_filename);
            let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
            if stem.is_empty() {
                p.source_sha256.clone()
            } else {
                stem.to_string()
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    sanitize_stem(&raw)
}

/// Keep ASCII alphanumerics, `-`, `_`, `.`; replace anything else with `_`.
fn sanitize_stem(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

/// Serialize the evidence artifact to a JSON string (test/inspection helper):
/// the same shape `emit_evidence_artifact` writes, without touching the
/// filesystem or the environment.
pub fn evidence_artifact_json(
    graph: &DocumentGraph,
    evidence: &SectionEvidence,
) -> Result<String, serde_json::Error> {
    // Deterministic ordering by node id keeps the test stable without relying
    // on text_order being set on synthetic fixtures.
    let mut by_id: BTreeMap<String, &NodeId> = BTreeMap::new();
    for (id, f) in evidence.per_node.iter() {
        if f.any() {
            by_id.insert(id.to_string(), id);
        }
    }
    let mut sections = Vec::new();
    for id in by_id.values() {
        let n = match graph.nodes.get(id) {
            Some(n) => n,
            None => continue,
        };
        let flags = &evidence.per_node[id];
        sections.push(EvidenceSection {
            node_id: id.to_string(),
            text_order: n.text_order,
            height_flag: flags.height_flag,
            overlap_flag: flags.overlap_flag,
            count_flag: flags.count_flag,
        });
    }
    let mut bad_fonts: Vec<String> = evidence.bad_fonts.iter().cloned().collect();
    bad_fonts.sort();
    let artifact = EvidenceArtifact {
        main_font: evidence.main_font.clone(),
        bad_fonts,
        sections,
    };
    serde_json::to_string_pretty(&artifact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphs::detectors::NodeFlags;
    use crate::graphs::graph_sanity::SanityReport;
    use crate::types::{BoundingBox, DocumentGraph, DocumentNode, PhysicalLocation, StyleMetadata};
    use uuid::Uuid;

    /// Build root → one Section with given font + bbox, plus N extra sections.
    /// Returns (graph, section ids in creation order).
    fn build_sections(specs: &[(&str, Option<&str>)]) -> (DocumentGraph, Vec<NodeId>) {
        let root_id = Uuid::new_v4();
        let mut graph = DocumentGraph::new_with_root(root_id);
        let mut root = DocumentNode::new_with_id(root_id, "Document", "root".into());
        root.location.semantic.depth = 0;
        let mut ids = Vec::new();
        for (i, (text, fam)) in specs.iter().enumerate() {
            let id = Uuid::new_v4();
            ids.push(id);
            root.children.push(id);
            let mut n = DocumentNode::new_with_id(id, "Section", (*text).into());
            n.parent = Some(root_id);
            n.text_order = Some(i as u32);
            n.location.semantic.depth = 1;
            n.location.physical = Some(PhysicalLocation {
                page: 1,
                bounding_box: BoundingBox {
                    x: 0.0,
                    y: (i as f32) * 20.0,
                    width: 100.0,
                    height: 12.0,
                },
            });
            n.style_info = Some(StyleMetadata {
                font_class: String::new(),
                font_size: None,
                is_bold: false,
                is_italic: false,
                font_family: fam.map(|s| s.to_string()),
                foreground_color: None,
                background_color: None,
            });
            graph.nodes.insert(id, n);
        }
        graph.nodes.insert(root_id, root);
        (graph, ids)
    }

    fn flagged_evidence(graph: &DocumentGraph, flag_ids: &[NodeId]) -> SectionEvidence {
        let mut ev = SectionEvidence::default();
        for id in flag_ids {
            let stem = crate::graphs::graph_sanity::font_stem(
                graph.nodes[id]
                    .style_info
                    .as_ref()
                    .and_then(|s| s.font_family.as_deref()),
            );
            ev.per_node.insert(
                *id,
                NodeFlags {
                    font_stem: stem,
                    height_flag: false,
                    overlap_flag: false,
                    count_flag: true,
                },
            );
        }
        ev.aggregate_verdicts(graph);
        ev
    }

    /// CR-71A — `prune_on_detection = false`: the prune step leaves the graph
    /// byte-identical (no-op) and records a summary with pruned == 0.
    #[test]
    fn test_prune_noop_leaves_graph_byte_identical() {
        let (mut graph, ids) = build_sections(&[
            ("1. Intro", Some("Times")),
            ("FLOPS", Some("DejaVuSans")),
            ("2. Body", Some("Times")),
        ]);
        let before = serde_json::to_string(&graph).unwrap();
        let evidence = flagged_evidence(&graph, &[ids[1]]);
        let mut report = SanityReport::default();
        let cfg = SectionPruneConfig {
            enabled: true,
            prune_on_detection: false,
            emit_evidence_artifact: false,
        };

        prune_sections(&mut graph, &evidence, &mut report, &cfg);

        let after = serde_json::to_string(&graph).unwrap();
        assert_eq!(before, after, "no-op prune must not change the graph");
        let s = report.section_prune.expect("summary recorded");
        assert_eq!(s.pruned, 0);
        assert_eq!(s.flagged, 1);
        assert_eq!(s.main_font.as_deref(), Some("times"));
        assert_eq!(s.bad_fonts, vec!["dejavusans".to_string()]);
    }

    /// CR-71B v1 — `prune_on_detection = true` demotes a flagged section whose
    /// font is confirmed bad (geo + font >= 2), leaving main-font sections alone.
    #[test]
    fn test_prune_on_detection_demotes_geo_plus_font() {
        let (mut graph, ids) = build_sections(&[
            ("1. Intro", Some("Times")),
            ("FLOPS", Some("DejaVuSans")),
            ("2. Body", Some("Times")),
        ]);
        // Flag the DejaVuSans figure callout (the geometric signal).
        let evidence = flagged_evidence(&graph, &[ids[1]]);
        assert!(
            evidence.bad_fonts.contains("dejavusans"),
            "dejavusans confirmed bad"
        );
        let mut report = SanityReport::default();
        let cfg = SectionPruneConfig {
            enabled: true,
            prune_on_detection: true,
            emit_evidence_artifact: false,
        };

        prune_sections(&mut graph, &evidence, &mut report, &cfg);

        // geo + font >= 2 demotes FLOPS; the main-font Times headers stay.
        assert_eq!(
            graph.nodes[&ids[1]].node_type, "Paragraph",
            "FLOPS (geo + bad font) demoted"
        );
        assert_eq!(
            graph.nodes[&ids[0]].node_type, "Section",
            "main-font header kept"
        );
        assert_eq!(
            graph.nodes[&ids[2]].node_type, "Section",
            "main-font header kept"
        );
        assert_eq!(report.section_prune.unwrap().pruned, 1);
    }

    /// CR-71A — the evidence artifact contains every flagged section with its
    /// attributes, plus the doc-level verdict.
    #[test]
    fn test_evidence_artifact_contains_every_flagged_section() {
        let (graph, ids) = build_sections(&[
            ("1. Intro", Some("Times")),
            ("FLOPS", Some("DejaVuSans")),
            ("2. Body", Some("Times")),
            ("1B", Some("DejaVuSans")),
        ]);
        // Flag the two DejaVuSans figure callouts.
        let evidence = flagged_evidence(&graph, &[ids[1], ids[3]]);
        let json = evidence_artifact_json(&graph, &evidence).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Doc-level verdict.
        assert_eq!(parsed["main_font"], "times");
        assert_eq!(parsed["bad_fonts"], serde_json::json!(["dejavusans"]));

        // Every flagged section present, by node_id, with its attributes.
        let secs = parsed["sections"].as_array().unwrap();
        assert_eq!(secs.len(), 2, "exactly the two flagged sections");
        let present: Vec<&str> = secs
            .iter()
            .map(|s| s["node_id"].as_str().unwrap())
            .collect();
        for id in [ids[1], ids[3]] {
            assert!(
                present.contains(&id.to_string().as_str()),
                "flagged section {id} in artifact"
            );
        }
        // Each carries the flag booleans + the text_order join key (text / font
        // / bbox are NOT duplicated — they live in the output graph JSON).
        for s in secs {
            assert_eq!(s["count_flag"], true);
            assert_eq!(s["height_flag"], false);
            assert!(s["text_order"].is_u64(), "text_order join key present");
        }
        // Unflagged Times sections are NOT in the artifact.
        for id in [ids[0], ids[2]] {
            assert!(!present.contains(&id.to_string().as_str()));
        }
    }
}
