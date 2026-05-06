//! Reading-Order Resort + Region Tagging — Block 06b.
//!
//! Annotates each `PdfTextElement` with a `region_label` and reorders the
//! element stream into reading order. Sits between
//! `AnalysisBuilder::finalize` and graph build:
//!
//! ```text
//! Tika → Vec<PdfTextElement>        (Tika emission order — possibly column-interleaved)
//!         │
//!         ▼
//! AnalysisBuilder::finalize         (font → geometry → region → page_stats → page_roles)
//!         │
//!         ▼
//! DocumentAnalysis                  (immutable after this point)
//!         │
//!         ▼
//! tag_and_resort(elements, &analysis)  ← THIS BLOCK
//!         │
//!         ▼
//! Vec<PdfTextElement>               (resorted, region-tagged)
//!         │
//!         ▼
//! Graph build → Rules engine
//! ```
//!
//! Per page the reading order is bucketed:
//!   `H-1, H-2, … (y-asc) → body leaves (depth-first) → F-1, F-2, … (y-asc) → orphans`.
//! Within each bucket, original Tika emission order is preserved (already
//! correct within a single column / region).
//!
//! Idempotent: the function reads `bbox`, `page_number`, etc. — never the
//! `region_label` it's about to set. Running twice produces byte-identical
//! output.

use std::collections::HashMap;

use crate::analytics::builder::DocumentAnalysis;
use crate::analytics::region::{PageRegions, Region};
use crate::types::PdfTextElement;

/// Tag each element with its region label and reorder the element stream
/// into reading order. Takes ownership of `elements` and returns a new Vec
/// in the resorted order.
///
/// Region labels follow `analytics::region::PageRegions.root` depth-first
/// leaf labels for body elements; per-page `H-1, H-2, …` for headers above
/// `analysis.geometry.header_y`; per-page `F-1, F-2, …` for footers below
/// `analysis.geometry.doc_footer_y`; and `None` for orphans (typically
/// rotated content or marginalia inside the body Y range but outside the
/// body X range).
pub fn tag_and_resort(
    elements: Vec<PdfTextElement>,
    analysis: &DocumentAnalysis,
) -> Vec<PdfTextElement> {
    if elements.is_empty() {
        return elements;
    }

    let header_y = analysis.geometry.header_y;
    let doc_footer_y = analysis.geometry.doc_footer_y;

    // Per-page bucket-order side-table. Records labels in reading-order
    // sequence; orphans (None) are appended last after all named buckets.
    // String labels for body leaves and H-N / F-N do NOT lex-sort to reading
    // order in general, so we record the order explicitly here.
    let mut bucket_order_per_page: HashMap<u32, Vec<Option<String>>> = HashMap::new();

    // Phase 1: Body element labeling — depth-first leaf walk per page.
    // For each leaf, set `region_label` on every element it indexes and
    // append the leaf label to that page's bucket order.
    let mut elements = elements;
    for regions in &analysis.region.per_page {
        let order = bucket_order_per_page
            .entry(regions.page_number)
            .or_default();
        label_body_leaves(&regions.root, regions, &mut elements, order);
    }

    // Phase 2: Header / footer labeling per page. Bucket non-body elements
    // by header / footer Y bands, then assign per-page H-N / F-N in y-asc
    // order. Elements that fall in neither band stay `None` (orphans).
    let mut headers_per_page: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut footers_per_page: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, el) in elements.iter().enumerate() {
        if el.placement.region_label.is_some() {
            // Body leaf already labeled in Phase 1 — never reclassify.
            continue;
        }
        let bbox = &el.placement.bounding_box;
        let bottom = bbox.y + bbox.height;
        let page = el.placement.page_number;
        // Only treat as header/footer if geometry actually identified the
        // band (header_y / doc_footer_y default to 0.0 when GeometryStats
        // has nothing to say — every element's bottom would then be > 0,
        // wrongly placing all orphans into the header bucket).
        if header_y > 0.0 && bottom <= header_y {
            headers_per_page.entry(page).or_default().push(i);
        } else if doc_footer_y > 0.0 && bbox.y >= doc_footer_y {
            footers_per_page.entry(page).or_default().push(i);
        }
    }

    // Collect all pages that need a bucket-order entry: any page that has
    // a Region tree, or that contributes a header / footer / orphan.
    let mut all_pages: Vec<u32> = bucket_order_per_page.keys().copied().collect();
    for page in headers_per_page
        .keys()
        .chain(footers_per_page.keys())
        .chain(elements.iter().map(|el| &el.placement.page_number))
    {
        if !bucket_order_per_page.contains_key(page) {
            bucket_order_per_page.insert(*page, Vec::new());
            all_pages.push(*page);
        }
    }
    all_pages.sort_unstable();
    all_pages.dedup();

    for page in &all_pages {
        let mut bucket_order = bucket_order_per_page.remove(page).unwrap_or_default();

        // Headers go FIRST — assign H-N in y-asc, prepend to bucket order.
        let mut header_labels: Vec<Option<String>> = Vec::new();
        if let Some(indices) = headers_per_page.get_mut(page) {
            indices.sort_by(|&a, &b| {
                elements[a]
                    .placement
                    .bounding_box
                    .y
                    .partial_cmp(&elements[b].placement.bounding_box.y)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for (n, &i) in indices.iter().enumerate() {
                let label = format!("H-{}", n + 1);
                elements[i].placement.region_label = Some(label.clone());
                header_labels.push(Some(label));
            }
        }
        if !header_labels.is_empty() {
            // Prepend.
            header_labels.append(&mut bucket_order);
            bucket_order = header_labels;
        }

        // Footers go AFTER body leaves — assign F-N in y-asc, append.
        if let Some(indices) = footers_per_page.get_mut(page) {
            indices.sort_by(|&a, &b| {
                elements[a]
                    .placement
                    .bounding_box
                    .y
                    .partial_cmp(&elements[b].placement.bounding_box.y)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for (n, &i) in indices.iter().enumerate() {
                let label = format!("F-{}", n + 1);
                elements[i].placement.region_label = Some(label.clone());
                bucket_order.push(Some(label));
            }
        }

        // Orphan bucket sentinel — always last.
        bucket_order.push(None);

        bucket_order_per_page.insert(*page, bucket_order);
    }

    // Phase 3: Resort — group element indices by page; bucket by region_label;
    // emit in (page-asc, bucket-order, original-index-order).
    let mut indices_per_page: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, el) in elements.iter().enumerate() {
        indices_per_page
            .entry(el.placement.page_number)
            .or_default()
            .push(i);
    }
    let mut sorted_pages: Vec<u32> = indices_per_page.keys().copied().collect();
    sorted_pages.sort_unstable();

    let mut new_order: Vec<usize> = Vec::with_capacity(elements.len());
    for page in &sorted_pages {
        let indices = indices_per_page
            .get(page)
            .expect("page must be present after bucketing");
        let empty: Vec<Option<String>> = vec![None];
        let bucket_order = bucket_order_per_page.get(page).unwrap_or(&empty);

        let mut buckets: HashMap<Option<String>, Vec<usize>> = HashMap::new();
        for &i in indices {
            buckets
                .entry(elements[i].placement.region_label.clone())
                .or_default()
                .push(i);
        }

        // Emit each named bucket in the recorded order; preserve
        // original-index order within the bucket (= Tika emission order,
        // already correct within a column / region).
        for label in bucket_order {
            if let Some(idxs) = buckets.remove(label) {
                new_order.extend(idxs);
            }
        }
        // Defensive: any leftover labels (shouldn't happen by construction)
        // get appended in original-index order.
        if !buckets.is_empty() {
            let mut leftover: Vec<usize> = buckets.into_values().flatten().collect();
            leftover.sort_unstable();
            new_order.extend(leftover);
        }
    }

    // Apply the permutation. Wrap each element in Option so we can move
    // exactly once per index without cloning.
    let mut slots: Vec<Option<PdfTextElement>> = elements.into_iter().map(Some).collect();
    let mut resorted: Vec<PdfTextElement> = Vec::with_capacity(slots.len());
    for i in new_order {
        resorted.push(
            slots[i]
                .take()
                .expect("permutation must visit each index exactly once"),
        );
    }
    resorted
}

/// Walk a Region tree depth-first; at each leaf, set `region_label` on
/// every indexed element and append the leaf label to `bucket_order`.
/// Body leaves are appended in the same order their indices reference
/// elements in `body_element_indices`.
fn label_body_leaves(
    region: &Region,
    page_regions: &PageRegions,
    elements: &mut [PdfTextElement],
    bucket_order: &mut Vec<Option<String>>,
) {
    if region.children.is_empty() {
        for &local_idx in &region.element_indices {
            let global_idx = page_regions.body_element_indices[local_idx as usize] as usize;
            elements[global_idx].placement.region_label = Some(region.label.clone());
        }
        bucket_order.push(Some(region.label.clone()));
    } else {
        for child in &region.children {
            label_body_leaves(child, page_regions, elements, bucket_order);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::geometry::{ColumnLayout, GeometryStats};
    use crate::analytics::region::{
        PageRegionDiagnostic, PageRegions, Region, RegionBox, RegionStats,
    };
    use crate::types::{BoundingBox, FontClass, Placement};

    // ── Synthetic-fixture helpers ──────────────────────────────────────────────

    fn mk_element(page: u32, x: f32, y: f32, w: f32, h: f32, text: &str) -> PdfTextElement {
        PdfTextElement {
            text: text.to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: "Times".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: "normal".to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: page,
                bounding_box: BoundingBox {
                    x,
                    y,
                    width: w,
                    height: h,
                },
                line_number: 0,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
                region_label: None,
                page_width: 0.0,
                page_height: 0.0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    fn mk_rotated(page: u32, x: f32, y: f32, w: f32, h: f32) -> PdfTextElement {
        let mut e = mk_element(page, x, y, w, h, "rotated");
        e.placement.rotation = 90;
        e
    }

    fn mk_geometry(header_y: f32, doc_footer_y: f32, left_x: f32, right_x: f32) -> GeometryStats {
        GeometryStats {
            header_y,
            doc_footer_y,
            left_x,
            right_x,
            column_layout: ColumnLayout {
                column_count: 1,
                column_dividers: vec![],
            },
            ..Default::default()
        }
    }

    /// Build a single-leaf PageRegions covering all body indices `0..n`.
    fn mk_single_leaf_page(page_number: u32, n_body: usize) -> PageRegions {
        let leaf = Region {
            r#box: RegionBox {
                x0: 0.0,
                y0: 0.0,
                x1: 600.0,
                y1: 800.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: (0..n_body as u32).collect(),
        };
        PageRegions {
            page_number,
            body_box: leaf.r#box,
            median_line_height: 12.0,
            body_element_indices: (0..n_body as u32).collect(),
            root: leaf,
            diagnostic: PageRegionDiagnostic::default(),
        }
    }

    /// Build a two-leaf PageRegions with a vertical cut. Left leaf takes
    /// `left_indices` (page-local positions in body_element_indices); right
    /// leaf takes the rest. `body_element_indices` is the document-wide
    /// global-index map for body elements on this page.
    fn mk_two_column_page(
        page_number: u32,
        body_element_indices: Vec<u32>,
        left_indices: Vec<u32>,
        right_indices: Vec<u32>,
    ) -> PageRegions {
        let left = Region {
            r#box: RegionBox {
                x0: 0.0,
                y0: 0.0,
                x1: 300.0,
                y1: 800.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: left_indices,
        };
        let right = Region {
            r#box: RegionBox {
                x0: 300.0,
                y0: 0.0,
                x1: 600.0,
                y1: 800.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "2".to_string(),
            element_indices: right_indices,
        };
        let root = Region {
            r#box: RegionBox {
                x0: 0.0,
                y0: 0.0,
                x1: 600.0,
                y1: 800.0,
            },
            axis: Some(crate::analytics::region::CutAxis::V),
            cut_coords: vec![300.0],
            children: vec![left, right],
            label: "root".to_string(),
            element_indices: vec![],
        };
        PageRegions {
            page_number,
            body_box: root.r#box,
            median_line_height: 12.0,
            body_element_indices,
            root,
            diagnostic: PageRegionDiagnostic::default(),
        }
    }

    fn mk_analysis(geometry: GeometryStats, region_pages: Vec<PageRegions>) -> DocumentAnalysis {
        let n = region_pages.len() as u32;
        DocumentAnalysis {
            font: Default::default(),
            geometry,
            page_stats: Default::default(),
            region: RegionStats {
                per_page: region_pages,
                source_pages: n,
            },
        }
    }

    // ── 1. Single-column body — labels assigned, order preserved ──────────────

    #[test]
    fn single_column_labels_and_order() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        let elements: Vec<_> = (0..5)
            .map(|i| mk_element(1, 110.0, 80.0 + (i as f32) * 14.0, 380.0, 14.0, "body"))
            .collect();
        let analysis = mk_analysis(geo, vec![mk_single_leaf_page(1, 5)]);

        let out = tag_and_resort(elements.clone(), &analysis);
        assert_eq!(out.len(), 5);
        for el in &out {
            assert_eq!(el.placement.region_label.as_deref(), Some("1"));
        }
        // Order unchanged.
        for (a, b) in out.iter().zip(elements.iter()) {
            assert!((a.placement.bounding_box.y - b.placement.bounding_box.y).abs() < 1e-3);
        }
    }

    // ── 2. Two-column body — resort fixes Tika interleave ─────────────────────

    #[test]
    fn two_column_resort_fixes_interleave() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        // Tika interleaves: [L1, R1, L2, R2, L3, R3]. L is x=110, R is x=400.
        let elements = vec![
            mk_element(1, 110.0, 100.0, 150.0, 14.0, "L1"),
            mk_element(1, 400.0, 100.0, 150.0, 14.0, "R1"),
            mk_element(1, 110.0, 120.0, 150.0, 14.0, "L2"),
            mk_element(1, 400.0, 120.0, 150.0, 14.0, "R2"),
            mk_element(1, 110.0, 140.0, 150.0, 14.0, "L3"),
            mk_element(1, 400.0, 140.0, 150.0, 14.0, "R3"),
        ];
        // body_element_indices: 0..6; left column local indices: [0, 2, 4]
        // (L1, L2, L3); right column local indices: [1, 3, 5] (R1, R2, R3).
        let page = mk_two_column_page(1, (0..6).collect(), vec![0, 2, 4], vec![1, 3, 5]);
        let analysis = mk_analysis(geo, vec![page]);

        let out = tag_and_resort(elements, &analysis);
        let labels: Vec<_> = out
            .iter()
            .map(|e| e.placement.region_label.as_deref().unwrap_or(""))
            .collect();
        let texts: Vec<_> = out.iter().map(|e| e.text.as_str()).collect();

        // All left-column elements (label "1") must precede all right-column
        // (label "2"); order within each column preserved.
        assert_eq!(labels, vec!["1", "1", "1", "2", "2", "2"]);
        assert_eq!(texts, vec!["L1", "L2", "L3", "R1", "R2", "R3"]);
    }

    // ── 3. Header element gets H-1 and sorts first ────────────────────────────

    #[test]
    fn header_labeled_and_sorts_first() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        // 1 header above header_y, 3 body lines.
        let elements = vec![
            mk_element(1, 250.0, 30.0, 100.0, 12.0, "header"),
            mk_element(1, 110.0, 80.0, 380.0, 14.0, "body1"),
            mk_element(1, 110.0, 100.0, 380.0, 14.0, "body2"),
            mk_element(1, 110.0, 120.0, 380.0, 14.0, "body3"),
        ];
        // body_element_indices excludes the header (global idx 0); body
        // global idxs are 1, 2, 3.
        let leaf = Region {
            r#box: RegionBox {
                x0: 0.0,
                y0: 0.0,
                x1: 600.0,
                y1: 800.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: vec![0, 1, 2],
        };
        let page = PageRegions {
            page_number: 1,
            body_box: leaf.r#box,
            median_line_height: 14.0,
            body_element_indices: vec![1, 2, 3],
            root: leaf,
            diagnostic: PageRegionDiagnostic::default(),
        };
        let analysis = mk_analysis(geo, vec![page]);

        let out = tag_and_resort(elements, &analysis);
        assert_eq!(out[0].text, "header");
        assert_eq!(out[0].placement.region_label.as_deref(), Some("H-1"));
        for el in &out[1..] {
            assert_eq!(el.placement.region_label.as_deref(), Some("1"));
        }
    }

    // ── 4. Footer element gets F-1 and sorts last ─────────────────────────────

    #[test]
    fn footer_labeled_and_sorts_last() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        let elements = vec![
            mk_element(1, 110.0, 80.0, 380.0, 14.0, "body1"),
            mk_element(1, 110.0, 100.0, 380.0, 14.0, "body2"),
            mk_element(1, 110.0, 120.0, 380.0, 14.0, "body3"),
            mk_element(1, 250.0, 720.0, 100.0, 12.0, "footer"),
        ];
        let leaf = Region {
            r#box: RegionBox {
                x0: 0.0,
                y0: 0.0,
                x1: 600.0,
                y1: 800.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: vec![0, 1, 2],
        };
        let page = PageRegions {
            page_number: 1,
            body_box: leaf.r#box,
            median_line_height: 14.0,
            body_element_indices: vec![0, 1, 2],
            root: leaf,
            diagnostic: PageRegionDiagnostic::default(),
        };
        let analysis = mk_analysis(geo, vec![page]);

        let out = tag_and_resort(elements, &analysis);
        assert_eq!(out.last().unwrap().text, "footer");
        assert_eq!(
            out.last().unwrap().placement.region_label.as_deref(),
            Some("F-1")
        );
    }

    // ── 5. Multiple headers labeled H-1, H-2 in y-asc order ───────────────────

    #[test]
    fn multiple_headers_y_ascending() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        // Tika emission order: emit higher header (y=30) AFTER lower (y=10)
        // — the function must reassign by y-asc, not by emission order.
        // Both header bottoms must satisfy `y + h <= header_y` (50.0).
        let elements = vec![
            mk_element(1, 100.0, 30.0, 100.0, 10.0, "header_y30"),
            mk_element(1, 100.0, 10.0, 100.0, 10.0, "header_y10"),
            mk_element(1, 110.0, 100.0, 380.0, 14.0, "body"),
        ];
        let leaf = Region {
            r#box: RegionBox {
                x0: 0.0,
                y0: 0.0,
                x1: 600.0,
                y1: 800.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: vec![0],
        };
        let page = PageRegions {
            page_number: 1,
            body_box: leaf.r#box,
            median_line_height: 14.0,
            body_element_indices: vec![2],
            root: leaf,
            diagnostic: PageRegionDiagnostic::default(),
        };
        let analysis = mk_analysis(geo, vec![page]);

        let out = tag_and_resort(elements, &analysis);
        // Locate each by text, then check labels and resort order.
        let h_y10 = out.iter().find(|e| e.text == "header_y10").unwrap();
        let h_y30 = out.iter().find(|e| e.text == "header_y30").unwrap();
        assert_eq!(h_y10.placement.region_label.as_deref(), Some("H-1"));
        assert_eq!(h_y30.placement.region_label.as_deref(), Some("H-2"));
        // First two items in the resorted stream are the headers in y-asc.
        assert_eq!(out[0].text, "header_y10");
        assert_eq!(out[1].text, "header_y30");
    }

    // ── 6. Multi-page: H-1 / F-1 reset per page ───────────────────────────────

    #[test]
    fn multi_page_h_and_f_reset_per_page() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        let elements = vec![
            // Page 1
            mk_element(1, 100.0, 30.0, 100.0, 12.0, "p1_header"),
            mk_element(1, 110.0, 100.0, 380.0, 14.0, "p1_body"),
            mk_element(1, 100.0, 720.0, 100.0, 12.0, "p1_footer"),
            // Page 2
            mk_element(2, 100.0, 30.0, 100.0, 12.0, "p2_header"),
            mk_element(2, 110.0, 100.0, 380.0, 14.0, "p2_body"),
            mk_element(2, 100.0, 720.0, 100.0, 12.0, "p2_footer"),
        ];
        // body_element_indices excludes headers (global 0, 3) and footers
        // (global 2, 5); body indices on each page: [1] and [4].
        let p1_leaf = Region {
            r#box: RegionBox {
                x0: 0.0,
                y0: 0.0,
                x1: 600.0,
                y1: 800.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: vec![0],
        };
        let p2_leaf = p1_leaf.clone();
        let page1 = PageRegions {
            page_number: 1,
            body_box: p1_leaf.r#box,
            median_line_height: 14.0,
            body_element_indices: vec![1],
            root: p1_leaf,
            diagnostic: PageRegionDiagnostic::default(),
        };
        let page2 = PageRegions {
            page_number: 2,
            body_box: p2_leaf.r#box,
            median_line_height: 14.0,
            body_element_indices: vec![4],
            root: p2_leaf,
            diagnostic: PageRegionDiagnostic::default(),
        };
        let analysis = mk_analysis(geo, vec![page1, page2]);

        let out = tag_and_resort(elements, &analysis);
        // Locate by text and check labels.
        let by_text = |t: &str| {
            out.iter()
                .find(|e| e.text == t)
                .unwrap()
                .placement
                .region_label
                .clone()
        };
        assert_eq!(by_text("p1_header").as_deref(), Some("H-1"));
        assert_eq!(by_text("p1_footer").as_deref(), Some("F-1"));
        assert_eq!(by_text("p2_header").as_deref(), Some("H-1"));
        assert_eq!(by_text("p2_footer").as_deref(), Some("F-1"));
        // Page-1 elements come before page-2 elements in the resorted stream.
        let page_seq: Vec<u32> = out.iter().map(|e| e.placement.page_number).collect();
        assert_eq!(page_seq, vec![1, 1, 1, 2, 2, 2]);
    }

    // ── 7. Orphan element (in-body Y, out-of-body X) — None, sorts last ───────

    #[test]
    fn orphan_label_none_and_sorts_last_on_page() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        // 3 body elements (in-body X) + 1 sidebar element at x=50 (out of
        // body X range) but with y in body range. Sidebar is excluded from
        // body_element_indices — it stays an orphan.
        let elements = vec![
            mk_element(1, 110.0, 100.0, 380.0, 14.0, "body1"),
            mk_element(1, 110.0, 120.0, 380.0, 14.0, "body2"),
            mk_element(1, 110.0, 140.0, 380.0, 14.0, "body3"),
            mk_element(1, 50.0, 110.0, 30.0, 14.0, "sidebar"),
        ];
        let leaf = Region {
            r#box: RegionBox {
                x0: 100.0,
                y0: 50.0,
                x1: 500.0,
                y1: 700.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: vec![0, 1, 2],
        };
        let page = PageRegions {
            page_number: 1,
            body_box: leaf.r#box,
            median_line_height: 14.0,
            body_element_indices: vec![0, 1, 2], // sidebar (idx 3) excluded
            root: leaf,
            diagnostic: PageRegionDiagnostic::default(),
        };
        let analysis = mk_analysis(geo, vec![page]);

        let out = tag_and_resort(elements, &analysis);
        let sidebar = out.iter().find(|e| e.text == "sidebar").unwrap();
        assert!(sidebar.placement.region_label.is_none());
        assert_eq!(out.last().unwrap().text, "sidebar");
    }

    // ── 8. Idempotence — byte-identical output for the same (input, analysis)
    // pair across two invocations. The labeling reads element index slots in
    // `body_element_indices`, so chaining one output into the next call is NOT
    // a meaningful idempotence test — the analysis is built for the original
    // index ordering. The handoff's idempotence contract is determinism on
    // the same input, which is what we assert here.

    #[test]
    fn idempotent_two_runs_same_input() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        let elements = vec![
            mk_element(1, 110.0, 100.0, 150.0, 14.0, "L1"),
            mk_element(1, 400.0, 100.0, 150.0, 14.0, "R1"),
            mk_element(1, 110.0, 120.0, 150.0, 14.0, "L2"),
            mk_element(1, 400.0, 120.0, 150.0, 14.0, "R2"),
        ];
        let page = mk_two_column_page(1, (0..4).collect(), vec![0, 2], vec![1, 3]);
        let analysis = mk_analysis(geo, vec![page]);

        let first = tag_and_resort(elements.clone(), &analysis);
        let second = tag_and_resort(elements.clone(), &analysis);
        let first_json = serde_json::to_string(&first).unwrap();
        let second_json = serde_json::to_string(&second).unwrap();
        assert_eq!(
            first_json, second_json,
            "two calls with the same input must produce byte-identical output"
        );
    }

    // ── 9. Full bucket sequence: H → body → F → orphan ────────────────────────

    #[test]
    fn full_bucket_sequence_one_page() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        let elements = vec![
            mk_element(1, 100.0, 30.0, 100.0, 12.0, "header"),
            mk_element(1, 110.0, 100.0, 150.0, 14.0, "L1"),
            mk_element(1, 400.0, 100.0, 150.0, 14.0, "R1"),
            mk_element(1, 110.0, 120.0, 150.0, 14.0, "L2"),
            mk_element(1, 400.0, 120.0, 150.0, 14.0, "R2"),
            mk_element(1, 100.0, 720.0, 100.0, 12.0, "footer"),
            mk_element(1, 50.0, 200.0, 30.0, 14.0, "orphan"),
        ];
        // Body elements (in-body X): L1, R1, L2, R2 → global indices 1,2,3,4.
        let page = mk_two_column_page(1, vec![1, 2, 3, 4], vec![0, 2], vec![1, 3]);
        let analysis = mk_analysis(geo, vec![page]);

        let out = tag_and_resort(elements, &analysis);
        let texts: Vec<_> = out.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["header", "L1", "L2", "R1", "R2", "footer", "orphan"]
        );
        let labels: Vec<_> = out
            .iter()
            .map(|e| e.placement.region_label.clone())
            .collect();
        assert_eq!(
            labels,
            vec![
                Some("H-1".to_string()),
                Some("1".to_string()),
                Some("1".to_string()),
                Some("2".to_string()),
                Some("2".to_string()),
                Some("F-1".to_string()),
                None,
            ]
        );
    }

    // ── 10. Empty input doesn't panic ─────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        let analysis = mk_analysis(geo, vec![]);
        let out = tag_and_resort(vec![], &analysis);
        assert!(out.is_empty());
    }

    // ── 11. Page with no PageRegions entry — all-orphan, Tika order ──────────

    #[test]
    fn page_without_region_tree_falls_through_as_orphans() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        let elements = vec![
            mk_element(1, 110.0, 100.0, 380.0, 14.0, "a"),
            mk_element(1, 110.0, 120.0, 380.0, 14.0, "b"),
            mk_element(1, 110.0, 140.0, 380.0, 14.0, "c"),
        ];
        // Empty per_page — analytics produced no Region tree for this page.
        let analysis = mk_analysis(geo, vec![]);
        let out = tag_and_resort(elements, &analysis);
        for el in &out {
            assert!(el.placement.region_label.is_none());
        }
        let texts: Vec<_> = out.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["a", "b", "c"], "Tika order preserved");
    }

    // ── 13. Rotated elements bypass the body branch ───────────────────────────

    #[test]
    fn rotated_element_not_body_labeled() {
        let geo = mk_geometry(50.0, 700.0, 100.0, 500.0);
        // Rotated sidebar inside the body Y range. RegionStatsBuilder excludes
        // it from body_element_indices upstream, so we model that here.
        let elements = vec![
            mk_element(1, 110.0, 100.0, 380.0, 14.0, "body1"),
            mk_rotated(1, 50.0, 200.0, 30.0, 100.0),
            mk_element(1, 110.0, 120.0, 380.0, 14.0, "body2"),
        ];
        let leaf = Region {
            r#box: RegionBox {
                x0: 100.0,
                y0: 50.0,
                x1: 500.0,
                y1: 700.0,
            },
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: vec![0, 1],
        };
        let page = PageRegions {
            page_number: 1,
            body_box: leaf.r#box,
            median_line_height: 14.0,
            body_element_indices: vec![0, 2], // skip the rotated sidebar (global 1)
            root: leaf,
            diagnostic: PageRegionDiagnostic::default(),
        };
        let analysis = mk_analysis(geo, vec![page]);

        let out = tag_and_resort(elements, &analysis);
        let rotated = out
            .iter()
            .find(|e| e.placement.rotation == 90)
            .expect("rotated element present in output");
        assert!(
            rotated.placement.region_label.is_none(),
            "rotated element with body-Y bbox stays orphan when excluded from body_element_indices"
        );
    }
}
