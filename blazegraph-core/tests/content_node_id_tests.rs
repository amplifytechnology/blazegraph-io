//! CR-83 acceptance tests: content + breadcrumb + occurrence node IDs.
//!
//! These pin the CR-83 "Bullet test" (acceptance) and the three
//! "open details to smoke out with tests" decisions, exercised through
//! the real generic-markdown parse path (so breadcrumb derivation runs
//! against actual section nesting, not synthetic shortcuts).
//!
//! Mental model for the assertions: each body node has a `text_order`
//! (stable, positional — emission order) and an `id` (content-derived,
//! edit-stable). We compare the *set* / *map* of IDs across edits. The
//! `text_order → id` map is the natural lens: "which node, at which
//! emission slot, kept its id".

use blazegraph_io_core::graphs::serialization::canonical::graph_sha256;
use blazegraph_io_core::preprocessors::md::{generic_md, ParseOptions};
use blazegraph_io_core::types::*;
use std::collections::{HashMap, HashSet};

// =========================================================================
// Helpers
// =========================================================================

fn parse(input: &str) -> DocumentGraph {
    generic_md::parse(input, ParseOptions::default())
        .expect("markdown parses")
        .graph
}

/// `text_order → (node_type, content_text)` — the body shape, for asserting
/// which slot a node lives in independent of its id.
fn shape_by_order(g: &DocumentGraph) -> HashMap<u32, (String, String)> {
    g.nodes
        .values()
        .filter_map(|n| {
            n.text_order
                .map(|t| (t, (n.node_type.clone(), n.content.text.clone())))
        })
        .collect()
}

/// All body node IDs as a set.
fn id_set(g: &DocumentGraph) -> HashSet<NodeId> {
    g.nodes
        .values()
        .filter_map(|n| n.text_order.map(|_| n.id))
        .collect()
}

/// Find the id of the (first) body node whose content text matches.
fn id_of_text(g: &DocumentGraph, text: &str) -> NodeId {
    g.nodes
        .values()
        .find(|n| n.text_order.is_some() && n.content.text == text)
        .unwrap_or_else(|| panic!("no body node with text {text:?}"))
        .id
}

// =========================================================================
// Bullet test #1 — the kicker: byte-identical paragraphs → distinct IDs,
//                  graph_sha256 round-trip holds (no collision / data loss).
// =========================================================================

#[test]
fn kicker_identical_paragraphs_get_distinct_ids_no_collision() {
    let md = "\
# Intro

Same body.

Same body.
";
    let g = parse(md);

    // Two byte-identical paragraphs under the same breadcrumb.
    let para_ids: Vec<NodeId> = {
        let mut v: Vec<&DocumentNode> = g
            .nodes
            .values()
            .filter(|n| n.node_type == "Paragraph" && n.content.text == "Same body.")
            .collect();
        v.sort_by_key(|n| n.text_order.unwrap());
        v.iter().map(|n| n.id).collect()
    };
    assert_eq!(para_ids.len(), 2, "expected two identical paragraphs");
    assert_ne!(
        para_ids[0], para_ids[1],
        "byte-identical paragraphs must occurrence-disambiguate to distinct IDs"
    );

    // No collision / data loss: every body node has a unique id, so the
    // HashMap<NodeId, Node> graph keeps all of them.
    let body_count = g.nodes.values().filter(|n| n.text_order.is_some()).count();
    let unique_ids = id_set(&g).len();
    assert_eq!(
        body_count, unique_ids,
        "all body nodes must have distinct IDs (no collision)"
    );

    // graph_sha256 is deterministic across a fresh reparse of the same bytes.
    let g2 = parse(md);
    assert_eq!(
        graph_sha256(&g),
        graph_sha256(&g2),
        "graph_sha256 must be stable across reparse of identical bytes"
    );
}

// =========================================================================
// Bullet test #2 — edit locality: edit one paragraph → only that node's
//                  id changes; siblings / cousins / sections stable.
// =========================================================================

#[test]
fn edit_locality_only_edited_paragraph_rotates() {
    let before = "\
# Alpha

First paragraph.

Second paragraph.

# Beta

Third paragraph.
";
    let after = "\
# Alpha

First paragraph EDITED.

Second paragraph.

# Beta

Third paragraph.
";
    let g1 = parse(before);
    let g2 = parse(after);

    // Stable nodes: every node whose content + breadcrumb is unchanged.
    for text in ["Alpha", "Second paragraph.", "Beta", "Third paragraph."] {
        assert_eq!(
            id_of_text(&g1, text),
            id_of_text(&g2, text),
            "node {text:?} should keep its id across an unrelated edit"
        );
    }

    // The edited paragraph's id is new (the old one is gone, the new text
    // has an id that did not exist before).
    let old_para = id_of_text(&g1, "First paragraph.");
    let new_para = id_of_text(&g2, "First paragraph EDITED.");
    assert_ne!(old_para, new_para, "the edited paragraph's id must rotate");
    assert!(
        !id_set(&g2).contains(&old_para),
        "the pre-edit paragraph id must no longer be present"
    );

    // Exactly one id changed: the symmetric difference of the id sets is
    // {old_para} ∪ {new_para}.
    let diff: HashSet<_> = id_set(&g1)
        .symmetric_difference(&id_set(&g2))
        .copied()
        .collect();
    assert_eq!(
        diff,
        HashSet::from([old_para, new_para]),
        "exactly one node id should change on a single-paragraph edit"
    );
}

// =========================================================================
// Bullet test #3 — insertion stability: insert a paragraph/section mid-doc
//                  → existing nodes' IDs unchanged (only the new node is new).
// =========================================================================

#[test]
fn insertion_stability_existing_ids_unchanged() {
    let before = "\
# Sec

P one.

P two.
";
    let after = "\
# Sec

P one.

P inserted.

P two.
";
    let g1 = parse(before);
    let g2 = parse(after);

    // Pre-existing nodes keep their ids despite the insertion shifting
    // text_order for everything after the insertion point.
    for text in ["Sec", "P one.", "P two."] {
        assert_eq!(
            id_of_text(&g1, text),
            id_of_text(&g2, text),
            "node {text:?} must keep its id across an insertion (positional shift must not rotate it)"
        );
    }

    // The new graph's ids are exactly the old set plus the inserted node.
    let added: HashSet<_> = id_set(&g2).difference(&id_set(&g1)).copied().collect();
    assert_eq!(added, HashSet::from([id_of_text(&g2, "P inserted.")]));
    assert!(
        id_set(&g1).is_subset(&id_set(&g2)),
        "every pre-insertion id must survive"
    );
}

// =========================================================================
// Bullet test #4 — reorder stability: reorder two siblings → IDs unchanged.
// =========================================================================

#[test]
fn reorder_stability_sibling_ids_unchanged() {
    let before = "\
# Sec

Apple.

Banana.
";
    let after = "\
# Sec

Banana.

Apple.
";
    let g1 = parse(before);
    let g2 = parse(after);

    // Same id set; only emission order (text_order) differs.
    assert_eq!(
        id_set(&g1),
        id_set(&g2),
        "reordering siblings must not change any id"
    );

    // The two paragraphs swapped text_order slots but kept their ids.
    let apple1 = id_of_text(&g1, "Apple.");
    let banana1 = id_of_text(&g1, "Banana.");
    assert_eq!(apple1, id_of_text(&g2, "Apple."));
    assert_eq!(banana1, id_of_text(&g2, "Banana."));

    // Confirm the slots actually swapped (so we know this was a real reorder).
    let shape1 = shape_by_order(&g1);
    let shape2 = shape_by_order(&g2);
    assert_ne!(
        shape1, shape2,
        "the reorder must actually change emission slots"
    );
}

// =========================================================================
// Bullet test #5 / Open-detail #3 — heading-edit scope: rename a section →
//   ONLY that section's subtree rotates; everything else stable.
// =========================================================================

#[test]
fn heading_edit_rotates_only_renamed_subtree() {
    let before = "\
# Keep

Keep body.

# Rename me

Child body.

## Sub

Sub body.
";
    let after = "\
# Keep

Keep body.

# Renamed

Child body.

## Sub

Sub body.
";
    let g1 = parse(before);
    let g2 = parse(after);

    // The untouched subtree ("Keep" and its child) is fully stable.
    assert_eq!(id_of_text(&g1, "Keep"), id_of_text(&g2, "Keep"));
    assert_eq!(id_of_text(&g1, "Keep body."), id_of_text(&g2, "Keep body."));

    // The renamed section + its descendants ("Child body.", "Sub",
    // "Sub body.") all rotate, because their breadcrumb path changed.
    let rotated_old = [
        id_of_text(&g1, "Rename me"),
        id_of_text(&g1, "Child body."),
        id_of_text(&g1, "Sub"),
        id_of_text(&g1, "Sub body."),
    ];
    for old in rotated_old {
        assert!(
            !id_set(&g2).contains(&old),
            "every node in the renamed subtree must rotate (old id {old} must be gone)"
        );
    }

    // Scope assertion: the set of *removed* ids is EXACTLY the renamed
    // subtree (heading + 3 descendants), nothing else.
    let removed: HashSet<_> = id_set(&g1).difference(&id_set(&g2)).copied().collect();
    assert_eq!(
        removed,
        HashSet::from(rotated_old),
        "heading edit must rotate ONLY the renamed subtree — no collateral rotation"
    );
}

// =========================================================================
// Open-detail #1 — occurrence propagation to descendants of duplicate-
//   heading siblings. Two `## Example` siblings, each with a byte-identical
//   body, must yield FOUR distinct ids (2 headings + 2 bodies), because the
//   parent's occurrence folds into the child breadcrumb.
// =========================================================================

#[test]
fn duplicate_heading_siblings_keep_subtrees_unique() {
    let md = "\
# Top

## Example

Shared child body.

## Example

Shared child body.
";
    let g = parse(md);

    // No collision anywhere: 2 `Example` sections + 2 identical bodies + the
    // `Top` heading = 5 distinct body ids.
    let body_count = g.nodes.values().filter(|n| n.text_order.is_some()).count();
    assert_eq!(body_count, 5, "Top + 2 Example + 2 bodies");
    assert_eq!(
        id_set(&g).len(),
        body_count,
        "duplicate-heading siblings AND their identical children must all be unique"
    );

    // Specifically: the two `Shared child body.` paragraphs (same content,
    // same literal breadcrumb ["Top","Example"]) are disambiguated because
    // their parent's occurrence is folded into the breadcrumb they inherit.
    let bodies: Vec<NodeId> = {
        let mut v: Vec<&DocumentNode> = g
            .nodes
            .values()
            .filter(|n| n.content.text == "Shared child body.")
            .collect();
        v.sort_by_key(|n| n.text_order.unwrap());
        v.iter().map(|n| n.id).collect()
    };
    assert_eq!(bodies.len(), 2);
    assert_ne!(
        bodies[0], bodies[1],
        "children of duplicate-heading siblings must stay unique (parent-occurrence folded into breadcrumb)"
    );

    // graph_sha256 round-trips (faithful graph, no data loss).
    assert_eq!(graph_sha256(&g), graph_sha256(&parse(md)));
}

// =========================================================================
// Open-detail #2 — canonical_local_content + breadcrumb form (GOLDEN-ish).
//   Two assertions, encoding the chosen contract:
//   (a) node_type is NOT in the key: same text under the same breadcrumb,
//       differing only by classification, would collide — so the design
//       relies on breadcrumb/occurrence, not node_type. We pin the positive
//       direction: content discriminates and breadcrumb discriminates.
//   (b) breadcrumb canonical form is the section-heading path; a node's id
//       depends on its ancestor headings, NOT on its siblings or text_order.
// =========================================================================

#[test]
fn id_depends_on_content_and_breadcrumb_not_position() {
    // Same paragraph text under two different headings → different ids
    // (breadcrumb is part of the key).
    let md = "\
# H1

Body.

# H2

Body.
";
    let g = parse(md);
    let bodies: Vec<&DocumentNode> = {
        let mut v: Vec<&DocumentNode> = g
            .nodes
            .values()
            .filter(|n| n.content.text == "Body.")
            .collect();
        v.sort_by_key(|n| n.text_order.unwrap());
        v
    };
    assert_eq!(bodies.len(), 2);
    assert_ne!(
        bodies[0].id, bodies[1].id,
        "same content under different breadcrumbs must differ (breadcrumb in key)"
    );
    // These are NOT occurrence-disambiguated: their breadcrumb paths
    // differ (["H1"] vs ["H2"]), so each is occurrence 0 in its own bucket.
    // We can't read occurrence directly, but we can confirm the ids match a
    // *fresh* derivation that swaps nothing — i.e. they are stable.
    assert_eq!(graph_sha256(&g), graph_sha256(&parse(md)));
}

#[test]
fn id_is_position_independent_for_same_content_and_breadcrumb() {
    // The SAME single paragraph under the SAME heading gets the SAME id
    // regardless of how many unrelated nodes precede it (text_order shifts,
    // id does not).
    let a = "\
# H

Target.
";
    let b = "\
# H

Filler one.

Filler two.

Target.
";
    let ga = parse(a);
    let gb = parse(b);
    assert_eq!(
        id_of_text(&ga, "Target."),
        id_of_text(&gb, "Target."),
        "id must be independent of preceding-sibling count (text_order is not an id input)"
    );
}

// =========================================================================
// DT-10 (option C) — the document root ID is the content fingerprint of the
//   node set: recomputable from the node IDs alone (URD's derive-without-
//   Blazegraph), rotates on a content edit, stable across a reorder, and
//   collision-free across distinct docs (shared across byte-identical ones).
// =========================================================================

#[test]
fn root_id_is_content_fingerprint_of_node_set() {
    use blazegraph_io_core::graphs::NodeIdGenerator;

    fn root_of(g: &DocumentGraph) -> NodeId {
        g.nodes
            .values()
            .find(|n| n.node_type == "Document")
            .expect("document root present")
            .id
    }

    let base = "\
# H

Alpha.

Beta.
";
    let edited = "\
# H

Alpha EDITED.

Beta.
";
    let reordered = "\
# H

Beta.

Alpha.
";

    let g = parse(base);

    // (a) Derivable from the node set alone: the root equals the fingerprint
    //     re-computed from just the (non-root) node IDs — so URD can verify a
    //     doc's identity without re-parsing through Blazegraph.
    let body_ids: Vec<NodeId> = id_set(&g).into_iter().collect();
    assert_eq!(
        root_of(&g),
        NodeIdGenerator::root_id_from_nodes(&body_ids),
        "root must be the content fingerprint of its (non-root) node set"
    );

    // (b) Rotates on a content edit — the root is a doc-level fingerprint.
    assert_ne!(
        root_of(&g),
        root_of(&parse(edited)),
        "editing a node's content must rotate the document root"
    );

    // (c) Stable across a reorder — the sorted node-ID set is unchanged.
    assert_eq!(
        root_of(&g),
        root_of(&parse(reordered)),
        "reordering siblings must NOT rotate the root (sorted node-ID set)"
    );

    // (d) Byte-identical docs share a root (doc-level dedup signal); a
    //     different doc gets a different root (no cross-doc collision).
    assert_eq!(root_of(&g), root_of(&parse(base)));
    assert_ne!(root_of(&g), root_of(&parse(edited)));
}
