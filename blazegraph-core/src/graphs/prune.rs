//! CR-71A — The single section-prune step (the only graph mutator before the
//! CR-70 topology rebalance).
//!
//! In CR-71A the prune step is a **literal no-op**: even when its master switch
//! `prune_on_detection` is on, it does nothing to the graph. Its job is purely
//! *observation* — it writes a flagged-set summary into `SanityReport` (for the
//! existing diagnostic print) and, when `emit_evidence_artifact` is set, dumps
//! the per-doc `<doc>.evidence.json` debug artifact that feeds the CR-71B Python
//! pruning prototype.
//!
//! The mutating body (expand the flagged set to *similar* sections, protect real
//! headers, demote) is CR-71B, designed after the flagged set is observed. It
//! will slot into this same `prune_sections` interface behind the same
//! `prune_on_detection` gate. **Do not add pruning logic here in CR-71A.**

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
    /// Sections demoted by the prune body. **Always 0 in CR-71A** (no-op).
    pub pruned: usize,
    /// Document-level main-font verdict (plurality section font stem).
    pub main_font: Option<String>,
    /// Document-level bad-font verdict (flagged stems minus the main font).
    pub bad_fonts: Vec<String>,
    /// Whether the master mutate switch (`prune_on_detection`) was on. When
    /// false the step is flag + log + (debug) artifact only.
    pub prune_on_detection: bool,
}

/// CR-71A — the prune step. Slotted into `graph_sanity::apply()` between the
/// (parked-off) demote block and `rebalance_topology` — the only mutator slot.
///
/// CR-71A body: **no graph mutation**. Records the flagged-set summary into
/// `report.section_prune` and (when `cfg.emit_evidence_artifact`) writes the
/// evidence artifact. The `&mut DocumentGraph` is the locked final signature
/// (CR-71B mutates through it); CR-71A leaves it untouched — hence the
/// `needless_pass_by_ref_mut` allow (the `&mut` is the deliberate CR-71B
/// interface, not dead in the long run).
#[allow(clippy::needless_pass_by_ref_mut)]
pub fn prune_sections(
    graph: &mut DocumentGraph,
    evidence: &SectionEvidence,
    report: &mut crate::graphs::graph_sanity::SanityReport,
    cfg: &SectionPruneConfig,
) {
    let mut bad_fonts: Vec<String> = evidence.bad_fonts.iter().cloned().collect();
    bad_fonts.sort();

    let summary = SectionPruneSummary {
        flagged: evidence.per_node.values().filter(|f| f.any()).count(),
        // CR-71A is a no-op — nothing is demoted regardless of the gate.
        pruned: 0,
        main_font: evidence.main_font.clone(),
        bad_fonts,
        prune_on_detection: cfg.prune_on_detection,
    };

    if cfg.prune_on_detection {
        // CR-71B prune body slots in here, mutating `graph`. CR-71A: no-op.
        //
        // The `&mut DocumentGraph` is deliberately left untouched so that, with
        // the experiment config (old demoters off, detectors on,
        // prune_on_detection = true), the graph the CR-70 rebalance sees is
        // byte-identical to the no-detection path — the accepted CR-71A
        // regression. DO NOT add demotion/pruning logic here in CR-71A.
    }

    // Debug artifact (default off; never part of bgraph).
    if cfg.emit_evidence_artifact {
        if let Err(e) = emit_evidence_artifact(graph, evidence) {
            eprintln!("⚠️  CR-71A: failed to write evidence artifact: {e}");
        }
    }

    report.section_prune = Some(summary);
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
        .map(|(id, _)| (*id, graph.nodes.get(id).and_then(|n| n.text_order).unwrap_or(u32::MAX)))
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
    let json = serde_json::to_string_pretty(&artifact)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
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
    use crate::types::{
        BoundingBox, DocumentGraph, DocumentNode, PhysicalLocation, StyleMetadata,
    };
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
                bounding_box: BoundingBox { x: 0.0, y: (i as f32) * 20.0, width: 100.0, height: 12.0 },
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
                NodeFlags { font_stem: stem, height_flag: false, overlap_flag: false, count_flag: true },
            );
        }
        ev.aggregate_verdicts(graph);
        ev
    }

    /// CR-71A — `prune_on_detection = false`: the prune step leaves the graph
    /// byte-identical (no-op) and records a summary with pruned == 0.
    #[test]
    fn test_prune_noop_leaves_graph_byte_identical() {
        let (mut graph, ids) =
            build_sections(&[("1. Intro", Some("Times")), ("FLOPS", Some("DejaVuSans")), ("2. Body", Some("Times"))]);
        let before = serde_json::to_string(&graph).unwrap();
        let evidence = flagged_evidence(&graph, &[ids[1]]);
        let mut report = SanityReport::default();
        let cfg = SectionPruneConfig { enabled: true, prune_on_detection: false, emit_evidence_artifact: false };

        prune_sections(&mut graph, &evidence, &mut report, &cfg);

        let after = serde_json::to_string(&graph).unwrap();
        assert_eq!(before, after, "no-op prune must not change the graph");
        let s = report.section_prune.expect("summary recorded");
        assert_eq!(s.pruned, 0);
        assert_eq!(s.flagged, 1);
        assert_eq!(s.main_font.as_deref(), Some("times"));
        assert_eq!(s.bad_fonts, vec!["dejavusans".to_string()]);
    }

    /// CR-71A — `prune_on_detection = true` is STILL a no-op in CR-71A: graph
    /// stays byte-identical, pruned == 0.
    #[test]
    fn test_prune_on_detection_true_still_noop() {
        let (mut graph, ids) =
            build_sections(&[("1. Intro", Some("Times")), ("FLOPS", Some("DejaVuSans"))]);
        let before = serde_json::to_string(&graph).unwrap();
        let evidence = flagged_evidence(&graph, &[ids[1]]);
        let mut report = SanityReport::default();
        let cfg = SectionPruneConfig { enabled: true, prune_on_detection: true, emit_evidence_artifact: false };

        prune_sections(&mut graph, &evidence, &mut report, &cfg);

        assert_eq!(before, serde_json::to_string(&graph).unwrap(), "CR-71A prune body is a no-op even with the gate on");
        assert_eq!(report.section_prune.unwrap().pruned, 0);
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
        let present: Vec<&str> = secs.iter().map(|s| s["node_id"].as_str().unwrap()).collect();
        for id in [ids[1], ids[3]] {
            assert!(present.contains(&id.to_string().as_str()), "flagged section {id} in artifact");
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
