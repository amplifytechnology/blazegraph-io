// Per-page Region tree from XY-cut + retries + column-divider-aligned merge
// + bbox-crossing safety filter. Ported from `scripts/xy_cut_prototype.py`
// (canonical for algorithm behaviour). Block 03b of the document-analytics
// flow — see `docs/P2/core/handoffs/2026-05-04-xy-cut-section-detection-prototype.md`.
//
// The Region tree is the geometric prepass for section detection: leaves are
// structural units (section headers, paragraph blocks, figures, footnotes),
// and depth-first traversal yields reading order. Classification (which leaf
// is a section vs paragraph vs caption) is downstream — this pass is pure
// geometry.
//
// Body box: per Marcus 2026-05-06, uses `geometry.doc_footer_y` rather than
// `geometry.per_page_footer_y[p]`. Per-page footer is currently fragile
// (Tika size-uniformity + per-segment span granularity); doc-level gives a
// deterministic single rectangle. Easy revert when per-page detection gets
// its own CR — see `body_box_for_page` for the swap point.

use serde::{Deserialize, Serialize};

use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::{BoundingBox, PdfTextElement};

// ---------------------------------------------------------------------------
// Output type shape
// ---------------------------------------------------------------------------

/// One node in the partition tree (interior or leaf).
///
/// - Interior: `axis` is `Some(H|V)`, `cut_coords` is non-empty, `children`
///   is non-empty, `element_indices` is empty.
/// - Leaf: `axis` is `None`, `cut_coords` is empty, `children` is empty,
///   `element_indices` carries indices into the page's body-element list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Region {
    pub r#box: RegionBox,
    pub axis: Option<CutAxis>,
    pub cut_coords: Vec<f32>,
    pub children: Vec<Region>,
    /// Reading-order path label: `"1"`, `"2-1"`, `"2-1-3"`, etc. Filled by
    /// `label_tree` after the algorithm finishes. Empty until then.
    pub label: String,
    /// Page-local indices into `PageRegions.body_element_indices`. Each
    /// entry is itself an index into the document-wide `text_elements` list
    /// the analytics builder consumed. Resolving a leaf to elements:
    /// `for i in leaf.element_indices: page.body_element_indices[i]` →
    /// document-wide index → `preprocessor_output.text_elements[that]`.
    pub element_indices: Vec<u32>,
}

/// Per-page Region tree plus the body-element index map the leaves refer
/// to. The two fields are paired: leaf indices are positions in
/// `body_element_indices`, which themselves are document-wide element
/// indices (so consumers can dereference the original `PdfTextElement`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageRegions {
    /// 1-indexed page number on the source PDF.
    pub page_number: u32,
    /// Body box used for this page (same shape per page in the
    /// doc_footer_y formulation; varies if/when per-page footer is wired).
    pub body_box: RegionBox,
    /// Median line height of body elements on this page, the band-threshold
    /// driver for XY-cut.
    pub median_line_height: f32,
    /// Document-wide indices into the analytics builder's element stream.
    /// Already filtered: rotation == 0 + bbox overlaps body_box. Region
    /// leaves index into this list, not the global one.
    pub body_element_indices: Vec<u32>,
    /// The tree.
    pub root: Region,
    /// Per-page run telemetry — useful for debugging without re-running the
    /// algorithm.
    pub diagnostic: PageRegionDiagnostic,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageRegionDiagnostic {
    /// Number of v-cut subtrees collapsed by the column-divider-aligned merge.
    pub merged_subtrees: u32,
    /// Number of nodes collapsed by the bbox-crossing safety filter.
    pub bbox_filtered: u32,
}

/// Cut axis for an interior Region node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CutAxis {
    /// Horizontal cut: child boxes stack vertically (top-to-bottom reading order).
    H,
    /// Vertical cut: child boxes sit side-by-side (left-to-right reading order).
    V,
}

/// Inclusive-on-low, exclusive-on-high box in PDF/Tika point coordinates
/// (y increases downward).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RegionBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegionStats {
    /// Per-page Region trees in observation order. Empty when GeometryStats
    /// is not available in the FinalizationContext (the algorithm needs the
    /// body box and column dividers from geometry to run).
    pub per_page: Vec<PageRegions>,
    /// Number of pages the algorithm ran over.
    pub source_pages: u32,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tunable parameters for the Region tree construction. Defaults exactly
/// match `scripts/xy_cut_prototype.py` and were validated on the 6-PDF
/// corpus per the 2026-05-04 session.
#[derive(Debug, Clone)]
pub struct RegionStatsConfig {
    /// Absolute floor on band thickness in pt. Default: 8.0.
    pub abs_min_band_pt: f32,
    /// Multiple of median line height in box. Default: 1.2. Combined with
    /// `abs_min_band_pt` via `max(abs, rel × mlh)` to set the per-region
    /// band-thickness threshold.
    pub rel_min_band_line_heights: f32,
    /// Adaptive top-K filter: keep all bands whose thickness is at least
    /// this fraction of the largest band's thickness. Default: 0.60.
    /// Captures simultaneous structural breaks (e.g., a 3-row table split).
    pub top_k_fraction: f32,
    /// Threshold retry factors. The first attempt uses `1.0` (= the default
    /// threshold); if no cuts emerge, retry with progressively lower
    /// factors. Default: `[1.0, 0.75, 0.55, 0.4]` — 1 default + 3 retries.
    pub retry_factors: Vec<f32>,
    /// Explosion guard: if a *retry* attempt would produce more than this
    /// many cuts after the top-K filter, abandon and treat as a single
    /// block. Default: 3. The default attempt is never gated.
    pub max_cuts_at_retry: usize,
    /// Recursion depth cap. Default: 8. The algorithmic stop is "no bands";
    /// this is a safety floor.
    pub max_depth: usize,
    /// Tolerance (pt) on doc-level column-divider alignment. A v-cut whose
    /// any cut sits within ±`tolerance` of a divider preserves the entire
    /// node; otherwise the node collapses. Default: 15.0.
    pub column_divider_tolerance_pt: f32,
    /// Total band thickness perpendicular to the cut for the bbox-crossing
    /// safety filter. Default: 8.0 → cuts span ±4pt around the cut line.
    pub bbox_crossing_band_perp_pt: f32,
    /// Inset at each end of the cut (along its direction). Elements at the
    /// box edges (page numbers, marginalia) shouldn't invalidate a body
    /// cut just because they sit on the same line near the margin.
    /// Default: 5.0.
    pub bbox_crossing_band_inset_pt: f32,
}

impl Default for RegionStatsConfig {
    fn default() -> Self {
        Self {
            abs_min_band_pt: 8.0,
            rel_min_band_line_heights: 1.2,
            top_k_fraction: 0.60,
            retry_factors: vec![1.0, 0.75, 0.55, 0.4],
            max_cuts_at_retry: 3,
            max_depth: 8,
            column_divider_tolerance_pt: 15.0,
            bbox_crossing_band_perp_pt: 8.0,
            bbox_crossing_band_inset_pt: 5.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Per-element observation: just enough to run XY-cut without dragging the
/// full PdfTextElement (rotation filter applied at observe time; bbox
/// overlap with body box applied at finalize time).
#[derive(Debug, Clone)]
struct Observed {
    /// Index into the document-wide element stream the analytics builder
    /// consumed. Region leaves carry indices into a per-page subset of these.
    global_idx: u32,
    bbox: BoundingBox,
}

#[derive(Debug, Default)]
struct PageObservation {
    page_number: u32,
    width: f32,
    height: f32,
    elements: Vec<Observed>,
}

/// Builder for the per-page Region tree statistic.
///
/// Constructed once per document. Call [`observe`] for every
/// [`PdfTextElement`] in reading order, then [`finalize`] with a
/// FinalizationContext that has GeometryStats available.
#[derive(Debug, Default)]
pub struct RegionStatsBuilder {
    config: RegionStatsConfig,
    pages: Vec<PageObservation>,
    /// Document-wide element counter, incremented on every observe call
    /// (regardless of filtering). Stored on each Observed so leaves can
    /// resolve back to the original PdfTextElement.
    next_global_idx: u32,
}

impl RegionStatsBuilder {
    /// Construct a builder with explicit config. The `AnalysisBuilder` in
    /// `builder.rs` uses `RegionStatsBuilder::default()` which picks up
    /// `RegionStatsConfig::default()`.
    pub fn new(config: RegionStatsConfig) -> Self {
        Self {
            config,
            pages: Vec::new(),
            next_global_idx: 0,
        }
    }

    fn page_slot(&mut self, page_number: u32) -> usize {
        if let Some(idx) = self.pages.iter().position(|p| p.page_number == page_number) {
            return idx;
        }
        self.pages.push(PageObservation {
            page_number,
            width: 0.0,
            height: 0.0,
            elements: Vec::new(),
        });
        self.pages.len() - 1
    }
}

impl Statistic for RegionStatsBuilder {
    type Output = RegionStats;
    const NAME: &'static str = "region";

    fn observe(&mut self, element: &PdfTextElement) {
        let global_idx = self.next_global_idx;
        self.next_global_idx = self.next_global_idx.saturating_add(1);

        if element.rotation() != 0 {
            return;
        }

        let bbox = element.bounding_box().clone();
        let page_w = element.placement.page_width;
        let page_h = element.placement.page_height;
        let page_number = element.page_number();
        let idx = self.page_slot(page_number);
        let page = &mut self.pages[idx];

        // Page dimensions: prefer Tika's page-meta over bbox-extent (same
        // pattern as GeometryStatsBuilder.observe). Used for diagnostic /
        // future per-region column-detection work; xy_cut itself uses the
        // body box from GeometryStats, not page.{width,height}.
        if page_w > 0.0 {
            page.width = page_w;
        } else {
            let right = bbox.x + bbox.width;
            if right > page.width {
                page.width = right;
            }
        }
        if page_h > 0.0 {
            page.height = page_h;
        } else {
            let bottom = bbox.y + bbox.height;
            if bottom > page.height {
                page.height = bottom;
            }
        }

        page.elements.push(Observed { global_idx, bbox });
    }

    fn finalize(self, ctx: &FinalizationContext<'_>) -> Self::Output {
        let geometry = match ctx.geometry {
            Some(g) => g,
            // Without geometry we don't have a body box or column dividers
            // — emit empty output and let downstream fall back. AnalysisBuilder
            // wires the dependency correctly; this branch is for tests / other
            // direct callers that skip geometry.
            None => return RegionStats::default(),
        };

        let mut per_page = Vec::with_capacity(self.pages.len());
        for page in self.pages.iter() {
            per_page.push(finalize_page(page, geometry, &self.config));
        }
        let source_pages = per_page.len() as u32;
        RegionStats {
            per_page,
            source_pages,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-page finalize: body box → xy_cut → merge → bbox filter → label
// ---------------------------------------------------------------------------

fn finalize_page(
    page: &PageObservation,
    geometry: &crate::analytics::geometry::GeometryStats,
    config: &RegionStatsConfig,
) -> PageRegions {
    let body_box = body_box_for_page(geometry);
    // Filter to body elements: bbox overlaps body box. Rotation == 0 was
    // already enforced in observe.
    let mut body_indices_local: Vec<u32> = Vec::new();
    let mut body_global_indices: Vec<u32> = Vec::new();
    let mut body_bboxes: Vec<BoundingBox> = Vec::new();
    for o in &page.elements {
        if overlaps(&o.bbox, &body_box) {
            body_indices_local.push(body_indices_local.len() as u32);
            body_global_indices.push(o.global_idx);
            body_bboxes.push(o.bbox.clone());
        }
    }

    let mlh = median_line_height(&body_bboxes);

    // 1. XY-cut.
    let mut root = xy_cut(body_box, &body_bboxes, &body_indices_local, 0, mlh, config);

    // 2. Column-divider-aligned merge.
    let merged = merge_overfragmented(
        &mut root,
        &geometry.column_layout.column_dividers,
        config.column_divider_tolerance_pt,
    );

    // 3. Bbox-crossing safety filter.
    let bbox_filtered = remove_bbox_crossing_cuts(
        &mut root,
        &body_bboxes,
        config.bbox_crossing_band_perp_pt,
        config.bbox_crossing_band_inset_pt,
    );

    // 4. Reading-order labels.
    label_tree(&mut root, "");

    PageRegions {
        page_number: page.page_number,
        body_box,
        median_line_height: mlh,
        body_element_indices: body_global_indices,
        root,
        diagnostic: PageRegionDiagnostic {
            merged_subtrees: merged,
            bbox_filtered,
        },
    }
}

/// Body box for a page. Per Marcus 2026-05-06, uses `doc_footer_y` (not
/// `per_page_footer_y[p]`). When per-page footer detection gets its own CR
/// and becomes reliable, swap the `y1` here to:
///     `geometry.per_page_footer_y[p].unwrap_or(geometry.doc_footer_y)`
fn body_box_for_page(geometry: &crate::analytics::geometry::GeometryStats) -> RegionBox {
    RegionBox {
        x0: geometry.left_x,
        y0: geometry.header_y,
        x1: geometry.right_x,
        y1: geometry.doc_footer_y,
    }
}

fn overlaps(bbox: &BoundingBox, region: &RegionBox) -> bool {
    let x1 = bbox.x + bbox.width;
    let y1 = bbox.y + bbox.height;
    !(x1 <= region.x0 || bbox.x >= region.x1 || y1 <= region.y0 || bbox.y >= region.y1)
}

/// Median bbox height across body elements. Drives the per-region band
/// threshold via `rel_min_band_line_heights`. Defaults to 12.0 when no
/// elements (matches the prototype's fallback).
fn median_line_height(bboxes: &[BoundingBox]) -> f32 {
    let mut heights: Vec<f32> = bboxes
        .iter()
        .map(|b| b.height)
        .filter(|h| *h > 0.0)
        .collect();
    if heights.is_empty() {
        return 12.0;
    }
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    heights[heights.len() / 2]
}

// ---------------------------------------------------------------------------
// XY-cut core (pass 1)
// ---------------------------------------------------------------------------

/// Recursive whitespace-band partitioning with threshold retries.
///
/// Sweeps both axes for interior whitespace bands at a given threshold
/// factor; chooses the axis with the largest band; admits cuts via the
/// adaptive top-K filter; recurses on each sub-region. If the default
/// threshold finds nothing, retries with progressively lower factors. On
/// retry attempts only, an explosion guard rejects when too many cuts
/// emerge (a sign the threshold dipped into inter-line noise).
///
/// `bbox_indices` is parallel to `bboxes`: each entry in `bbox_indices` is
/// the local index that a leaf will record for that element.
fn xy_cut(
    region_box: RegionBox,
    bboxes: &[BoundingBox],
    bbox_indices: &[u32],
    depth: usize,
    mlh: f32,
    config: &RegionStatsConfig,
) -> Region {
    debug_assert_eq!(bboxes.len(), bbox_indices.len());

    if depth >= config.max_depth {
        return leaf(region_box, bbox_indices);
    }

    for (attempt, &factor) in config.retry_factors.iter().enumerate() {
        let h_bands = find_interior_bands(region_box, bboxes, CutAxis::H, mlh, factor, config);
        let v_bands = find_interior_bands(region_box, bboxes, CutAxis::V, mlh, factor, config);
        if h_bands.is_empty() && v_bands.is_empty() {
            continue;
        }

        let h_largest = h_bands.iter().map(|b| b.thickness).fold(0.0, f32::max);
        let v_largest = v_bands.iter().map(|b| b.thickness).fold(0.0, f32::max);
        let (chosen_axis, chosen_bands, chosen_max) = if h_largest >= v_largest {
            (CutAxis::H, h_bands, h_largest)
        } else {
            (CutAxis::V, v_bands, v_largest)
        };

        // Adaptive top-K filter: keep bands ≥ top_k_fraction × max.
        let mut kept: Vec<Band> = chosen_bands
            .into_iter()
            .filter(|b| b.thickness >= config.top_k_fraction * chosen_max)
            .collect();
        kept.sort_by(|a, b| {
            a.mid
                .partial_cmp(&b.mid)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Explosion guard — only on retry attempts.
        if attempt > 0 && kept.len() > config.max_cuts_at_retry {
            return leaf(region_box, bbox_indices);
        }

        let cut_coords: Vec<f32> = kept.iter().map(|b| b.mid).collect();
        let cut_coords = filter_cuts_with_content(region_box, bboxes, chosen_axis, &cut_coords);
        if cut_coords.is_empty() {
            // All cuts filtered — try a lower threshold (might reveal a
            // different separating band).
            continue;
        }

        let children = split_and_recurse(
            region_box,
            bboxes,
            bbox_indices,
            chosen_axis,
            &cut_coords,
            depth,
            mlh,
            config,
        );

        return Region {
            r#box: region_box,
            axis: Some(chosen_axis),
            cut_coords,
            children,
            label: String::new(),
            element_indices: Vec::new(),
        };
    }

    // Exhausted all retry factors with no usable cut → leaf.
    leaf(region_box, bbox_indices)
}

fn leaf(region_box: RegionBox, indices: &[u32]) -> Region {
    Region {
        r#box: region_box,
        axis: None,
        cut_coords: Vec::new(),
        children: Vec::new(),
        label: String::new(),
        element_indices: indices.to_vec(),
    }
}

#[allow(clippy::too_many_arguments)]
fn split_and_recurse(
    region_box: RegionBox,
    bboxes: &[BoundingBox],
    bbox_indices: &[u32],
    axis: CutAxis,
    cuts: &[f32],
    depth: usize,
    mlh: f32,
    config: &RegionStatsConfig,
) -> Vec<Region> {
    let mut children: Vec<Region> = Vec::with_capacity(cuts.len() + 1);
    let strips = strip_boxes(region_box, axis, cuts);
    for strip in strips {
        let (sub_bboxes, sub_indices) = bboxes_in(strip, bboxes, bbox_indices);
        children.push(xy_cut(
            strip,
            &sub_bboxes,
            &sub_indices,
            depth + 1,
            mlh,
            config,
        ));
    }
    children
}

fn strip_boxes(region_box: RegionBox, axis: CutAxis, cuts: &[f32]) -> Vec<RegionBox> {
    let mut strips: Vec<RegionBox> = Vec::with_capacity(cuts.len() + 1);
    match axis {
        CutAxis::H => {
            let mut prev = region_box.y0;
            for &c in cuts {
                strips.push(RegionBox {
                    x0: region_box.x0,
                    y0: prev,
                    x1: region_box.x1,
                    y1: c,
                });
                prev = c;
            }
            strips.push(RegionBox {
                x0: region_box.x0,
                y0: prev,
                x1: region_box.x1,
                y1: region_box.y1,
            });
        }
        CutAxis::V => {
            let mut prev = region_box.x0;
            for &c in cuts {
                strips.push(RegionBox {
                    x0: prev,
                    y0: region_box.y0,
                    x1: c,
                    y1: region_box.y1,
                });
                prev = c;
            }
            strips.push(RegionBox {
                x0: prev,
                y0: region_box.y0,
                x1: region_box.x1,
                y1: region_box.y1,
            });
        }
    }
    strips
}

fn bboxes_in(
    region: RegionBox,
    bboxes: &[BoundingBox],
    indices: &[u32],
) -> (Vec<BoundingBox>, Vec<u32>) {
    let mut out_b: Vec<BoundingBox> = Vec::new();
    let mut out_i: Vec<u32> = Vec::new();
    for (b, &i) in bboxes.iter().zip(indices.iter()) {
        if overlaps(b, &region) {
            out_b.push(b.clone());
            out_i.push(i);
        }
    }
    (out_b, out_i)
}

#[derive(Debug, Clone, Copy)]
struct Band {
    #[allow(dead_code)]
    start: f32,
    #[allow(dead_code)]
    end: f32,
    mid: f32,
    thickness: f32,
}

/// Find whitespace bands in `region_box` along the perpendicular axis.
/// Restricted to interior bands (strictly avoiding the box's outer edges)
/// whose thickness exceeds `max(factor × abs_min_band_pt,
/// factor × rel_min_band_line_heights × mlh)`.
fn find_interior_bands(
    region_box: RegionBox,
    bboxes: &[BoundingBox],
    axis: CutAxis,
    mlh: f32,
    factor: f32,
    config: &RegionStatsConfig,
) -> Vec<Band> {
    let (lo, hi) = match axis {
        CutAxis::H => (region_box.y0.round() as i32, region_box.y1.round() as i32),
        CutAxis::V => (region_box.x0.round() as i32, region_box.x1.round() as i32),
    };
    if hi <= lo {
        return Vec::new();
    }

    let span = (hi - lo) as usize;
    let mut filled = vec![false; span];
    for b in bboxes {
        let (a_raw, b_raw) = match axis {
            CutAxis::H => ((b.y).round() as i32, (b.y + b.height).round() as i32),
            CutAxis::V => ((b.x).round() as i32, (b.x + b.width).round() as i32),
        };
        let a = a_raw.max(lo);
        let bb = b_raw.min(hi);
        if bb <= a {
            continue;
        }
        filled[(a - lo) as usize..(bb - lo) as usize].fill(true);
    }

    // Whitespace runs.
    let mut runs: Vec<(i32, i32)> = Vec::new();
    let mut in_run = false;
    let mut run_start = lo;
    for (i, &f) in filled.iter().enumerate() {
        let pos = lo + i as i32;
        if !f && !in_run {
            in_run = true;
            run_start = pos;
        } else if f && in_run {
            in_run = false;
            runs.push((run_start, pos));
        }
    }
    if in_run {
        runs.push((run_start, hi));
    }

    let min_thickness =
        (factor * config.abs_min_band_pt).max(factor * config.rel_min_band_line_heights * mlh);

    let mut bands: Vec<Band> = Vec::new();
    for (start, end) in runs {
        // Interior only — skip bands touching the box's outer edge.
        if start <= lo || end >= hi {
            continue;
        }
        let thickness = (end - start) as f32;
        if thickness < min_thickness {
            continue;
        }
        bands.push(Band {
            start: start as f32,
            end: end as f32,
            mid: (start as f32 + end as f32) / 2.0,
            thickness,
        });
    }
    bands
}

/// Keep only cuts that produce non-empty strips on both sides.
fn filter_cuts_with_content(
    region_box: RegionBox,
    bboxes: &[BoundingBox],
    axis: CutAxis,
    cuts: &[f32],
) -> Vec<f32> {
    if cuts.is_empty() {
        return Vec::new();
    }
    let mut accepted: Vec<f32> = Vec::new();
    let mut prev = match axis {
        CutAxis::H => region_box.y0,
        CutAxis::V => region_box.x0,
    };
    for &c in cuts {
        let strip = match axis {
            CutAxis::H => RegionBox {
                x0: region_box.x0,
                y0: prev,
                x1: region_box.x1,
                y1: c,
            },
            CutAxis::V => RegionBox {
                x0: prev,
                y0: region_box.y0,
                x1: c,
                y1: region_box.y1,
            },
        };
        if bboxes.iter().any(|b| overlaps(b, &strip)) {
            accepted.push(c);
            prev = c;
        }
        // else: empty strip → drop this cut, prev unchanged (merge with next).
    }
    // Drop any trailing-empty-strip cuts.
    while let Some(&last) = accepted.last() {
        let tail = match axis {
            CutAxis::H => RegionBox {
                x0: region_box.x0,
                y0: last,
                x1: region_box.x1,
                y1: region_box.y1,
            },
            CutAxis::V => RegionBox {
                x0: last,
                y0: region_box.y0,
                x1: region_box.x1,
                y1: region_box.y1,
            },
        };
        if bboxes.iter().any(|b| overlaps(b, &tail)) {
            break;
        }
        accepted.pop();
    }
    accepted
}

// ---------------------------------------------------------------------------
// Pass 2: column-divider-aligned merge
// ---------------------------------------------------------------------------

/// Walk the tree bottom-up. For each v-cut node, collapse it iff none of
/// its cuts align with a doc-level column divider.
///
/// Returns the count of v-cut subtrees collapsed (telemetry).
fn merge_overfragmented(region: &mut Region, dividers: &[f32], tolerance: f32) -> u32 {
    let mut n = 0;
    for child in region.children.iter_mut() {
        n += merge_overfragmented(child, dividers, tolerance);
    }
    if matches!(region.axis, Some(CutAxis::V)) && !region.cut_coords.is_empty() {
        let any_aligned = region
            .cut_coords
            .iter()
            .any(|c| aligns_with_divider(*c, dividers, tolerance));
        if !any_aligned {
            collapse_to_leaf(region);
            n += 1;
        }
    }
    n
}

fn aligns_with_divider(cut: f32, dividers: &[f32], tolerance: f32) -> bool {
    dividers.iter().any(|d| (cut - *d).abs() <= tolerance)
}

fn collapse_to_leaf(region: &mut Region) {
    let indices = gather_leaf_indices(region);
    region.axis = None;
    region.cut_coords.clear();
    region.children.clear();
    region.element_indices = indices;
}

fn gather_leaf_indices(region: &Region) -> Vec<u32> {
    if region.children.is_empty() {
        return region.element_indices.clone();
    }
    let mut out: Vec<u32> = Vec::new();
    for child in &region.children {
        out.extend(gather_leaf_indices(child));
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 3: bbox-crossing safety filter
// ---------------------------------------------------------------------------

/// Walk the tree bottom-up. For each surviving cut, thicken into a band
/// (perpendicular ±perp/2, along-direction inset by along_inset on each
/// end) and test for AABB intersection with every element bbox in the
/// region. On the first hit, collapse the entire node into a leaf.
///
/// Returns the count of nodes collapsed (telemetry).
fn remove_bbox_crossing_cuts(
    region: &mut Region,
    all_bboxes: &[BoundingBox],
    perp: f32,
    along_inset: f32,
) -> u32 {
    let mut n = 0;
    for child in region.children.iter_mut() {
        n += remove_bbox_crossing_cuts(child, all_bboxes, perp, along_inset);
    }
    if region.children.is_empty() || region.cut_coords.is_empty() {
        return n;
    }

    // Gather only the bboxes that fall within this region's box. Keeping
    // this scoped per-node prevents marginalia from another part of the
    // page from triggering a collapse here.
    let region_bboxes: Vec<&BoundingBox> = all_bboxes
        .iter()
        .filter(|b| overlaps(b, &region.r#box))
        .collect();

    let half = perp / 2.0;
    let mut crossing = false;
    for &cut in &region.cut_coords {
        let band = match region.axis {
            Some(CutAxis::H) => RegionBox {
                x0: region.r#box.x0 + along_inset,
                y0: cut - half,
                x1: region.r#box.x1 - along_inset,
                y1: cut + half,
            },
            Some(CutAxis::V) => RegionBox {
                x0: cut - half,
                y0: region.r#box.y0 + along_inset,
                x1: cut + half,
                y1: region.r#box.y1 - along_inset,
            },
            None => continue,
        };
        for b in &region_bboxes {
            if overlaps(b, &band) {
                crossing = true;
                break;
            }
        }
        if crossing {
            break;
        }
    }

    if crossing {
        let indices = gather_leaf_indices(region);
        region.axis = None;
        region.cut_coords.clear();
        region.children.clear();
        region.element_indices = indices;
        n += 1;
    }
    n
}

// ---------------------------------------------------------------------------
// Pass 4: reading-order labels
// ---------------------------------------------------------------------------

fn label_tree(region: &mut Region, prefix: &str) {
    if region.children.is_empty() {
        region.label = if prefix.is_empty() {
            "1".to_string()
        } else {
            prefix.to_string()
        };
        return;
    }
    for (i, child) in region.children.iter_mut().enumerate() {
        let n = i + 1;
        let child_prefix = if prefix.is_empty() {
            n.to_string()
        } else {
            format!("{prefix}-{n}")
        };
        label_tree(child, &child_prefix);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::geometry::{ColumnLayout, GeometryStats};

    fn mk_bbox(x: f32, y: f32, w: f32, h: f32) -> BoundingBox {
        BoundingBox {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn mk_geometry(
        header_y: f32,
        doc_footer_y: f32,
        left_x: f32,
        right_x: f32,
        dividers: Vec<f32>,
    ) -> GeometryStats {
        GeometryStats {
            header_y,
            doc_footer_y,
            left_x,
            right_x,
            column_layout: ColumnLayout {
                column_count: (dividers.len() + 1) as u32,
                column_dividers: dividers,
            },
            ..Default::default()
        }
    }

    fn run(bboxes: Vec<BoundingBox>, geometry: &GeometryStats) -> PageRegions {
        let indices: Vec<u32> = (0..bboxes.len() as u32).collect();
        let cfg = RegionStatsConfig::default();
        let body_box = body_box_for_page(geometry);
        let body_bboxes: Vec<BoundingBox> = bboxes
            .iter()
            .filter(|b| overlaps(b, &body_box))
            .cloned()
            .collect();
        let body_indices: Vec<u32> = (0..body_bboxes.len() as u32).collect();
        let mlh = median_line_height(&body_bboxes);
        let mut root = xy_cut(body_box, &body_bboxes, &body_indices, 0, mlh, &cfg);
        let merged = merge_overfragmented(
            &mut root,
            &geometry.column_layout.column_dividers,
            cfg.column_divider_tolerance_pt,
        );
        let bbox_filtered = remove_bbox_crossing_cuts(
            &mut root,
            &body_bboxes,
            cfg.bbox_crossing_band_perp_pt,
            cfg.bbox_crossing_band_inset_pt,
        );
        label_tree(&mut root, "");
        PageRegions {
            page_number: 1,
            body_box,
            median_line_height: mlh,
            body_element_indices: indices,
            root,
            diagnostic: PageRegionDiagnostic {
                merged_subtrees: merged,
                bbox_filtered,
            },
        }
    }

    fn count_leaves(region: &Region) -> u32 {
        if region.children.is_empty() {
            1
        } else {
            region.children.iter().map(count_leaves).sum()
        }
    }

    fn collect_leaves<'a>(region: &'a Region, out: &mut Vec<&'a Region>) {
        if region.children.is_empty() {
            out.push(region);
        } else {
            for c in &region.children {
                collect_leaves(c, out);
            }
        }
    }

    // --- 1. Single-block body — no cuts ---------------------------------------
    #[test]
    fn single_block_yields_single_leaf() {
        // Solid body block: 20 lines of body text, no internal whitespace gaps
        // wide enough to trigger a cut.
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let mut bboxes = Vec::new();
        let mut y = 80.0;
        while y + 14.0 <= 700.0 {
            bboxes.push(mk_bbox(110.0, y, 380.0, 14.0));
            y += 14.0;
        }
        let pr = run(bboxes, &geometry);
        assert_eq!(count_leaves(&pr.root), 1);
        let mut leaves = Vec::new();
        collect_leaves(&pr.root, &mut leaves);
        assert_eq!(leaves[0].label, "1");
    }

    // --- 2. Two-section body — one h-cut --------------------------------------
    #[test]
    fn body_with_section_gap_splits_horizontally() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let mut bboxes = Vec::new();
        // Section 1 body at y=80..300, section 2 body at y=350..700.
        // Gap [300, 350) = 50pt — comfortably above abs_min_band_pt=8 and
        // 1.2 × line_height (14×1.2=16.8).
        let mut y = 80.0;
        while y + 14.0 <= 300.0 {
            bboxes.push(mk_bbox(110.0, y, 380.0, 14.0));
            y += 14.0;
        }
        y = 350.0;
        while y + 14.0 <= 700.0 {
            bboxes.push(mk_bbox(110.0, y, 380.0, 14.0));
            y += 14.0;
        }
        let pr = run(bboxes, &geometry);
        assert_eq!(pr.root.axis, Some(CutAxis::H));
        assert_eq!(pr.root.cut_coords.len(), 1);
        assert_eq!(count_leaves(&pr.root), 2);
        // Top section labeled "1", bottom "2" (h-cut → top-to-bottom).
        let mut leaves = Vec::new();
        collect_leaves(&pr.root, &mut leaves);
        assert_eq!(leaves[0].label, "1");
        assert_eq!(leaves[1].label, "2");
    }

    // --- 3. Two-column body with aligned divider — gutter survives merge ------
    #[test]
    fn two_column_with_divider_keeps_gutter() {
        // Doc divider at x=300 (gutter center). Body x ∈ [100, 500],
        // left col x ∈ [110, 280], right col x ∈ [320, 490], inter-col
        // gap [280, 320] = 40pt — well above abs_min_band_pt=8.
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![300.0]);
        let mut bboxes = Vec::new();
        let mut y = 80.0;
        while y + 14.0 <= 700.0 {
            bboxes.push(mk_bbox(110.0, y, 170.0, 14.0));
            bboxes.push(mk_bbox(320.0, y, 170.0, 14.0));
            y += 14.0;
        }
        let pr = run(bboxes, &geometry);
        // Root should be a v-cut at ~x=300, with 2 children.
        assert_eq!(pr.root.axis, Some(CutAxis::V));
        assert_eq!(pr.root.cut_coords.len(), 1);
        assert!(
            (pr.root.cut_coords[0] - 300.0).abs() < 15.0,
            "v-cut at {:?} not within tolerance of divider 300",
            pr.root.cut_coords
        );
        assert_eq!(pr.root.children.len(), 2);
        // No collapses on this page: aligned divider preserves the v-cut.
        assert_eq!(pr.diagnostic.merged_subtrees, 0);
    }

    // --- 4. Two-column body without divider in geometry — gutter collapses ----
    #[test]
    fn two_column_without_divider_collapses() {
        // Same body shape as test 3, but `column_dividers` is empty
        // (e.g., heatmap couldn't detect the gutter).
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let mut bboxes = Vec::new();
        let mut y = 80.0;
        while y + 14.0 <= 700.0 {
            bboxes.push(mk_bbox(110.0, y, 170.0, 14.0));
            bboxes.push(mk_bbox(320.0, y, 170.0, 14.0));
            y += 14.0;
        }
        let pr = run(bboxes, &geometry);
        // Without a divider to align against, the v-cut is "over-fragmentation"
        // and the merge collapses it into one leaf.
        assert_eq!(count_leaves(&pr.root), 1);
        assert_eq!(pr.diagnostic.merged_subtrees, 1);
    }

    // --- 5. Section-number split inside single column gets merged away --------
    #[test]
    fn inline_word_gap_gets_merged_away_in_single_column() {
        // Single-col page (no dividers). One section header line "1.
        // Introduction" with a 30pt inter-word gap will trigger a v-cut on
        // a retry threshold. Merge collapses it because no doc-level
        // divider aligns.
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let mut bboxes = Vec::new();
        // Header line: "1." at x=110, "Introduction" at x=145
        bboxes.push(mk_bbox(110.0, 80.0, 12.0, 14.0));
        bboxes.push(mk_bbox(145.0, 80.0, 100.0, 14.0));
        // Body lines below
        let mut y = 110.0;
        while y + 14.0 <= 700.0 {
            bboxes.push(mk_bbox(110.0, y, 380.0, 14.0));
            y += 14.0;
        }
        let pr = run(bboxes, &geometry);
        // Should collapse v-cuts; final tree may still have h-cuts but no
        // v-cuts that aren't aligned with the (empty) divider list.
        let mut walk = vec![&pr.root];
        let mut found_unaligned_v = false;
        while let Some(r) = walk.pop() {
            if r.axis == Some(CutAxis::V) {
                found_unaligned_v = true;
            }
            walk.extend(r.children.iter());
        }
        assert!(
            !found_unaligned_v,
            "single-col page must not retain any v-cut"
        );
    }

    // --- 6. Reading-order labels: depth-first, top-then-left ------------------
    #[test]
    fn labels_follow_depth_first_top_then_left() {
        // Two h-strips, top strip is single-col, bottom strip is two-col with
        // aligned divider.
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![300.0]);
        let mut bboxes = Vec::new();
        // Top single-col block y=[80, 300]
        let mut y = 80.0;
        while y + 14.0 <= 300.0 {
            bboxes.push(mk_bbox(110.0, y, 380.0, 14.0));
            y += 14.0;
        }
        // Bottom two-col block y=[350, 700]
        y = 350.0;
        while y + 14.0 <= 700.0 {
            bboxes.push(mk_bbox(110.0, y, 170.0, 14.0));
            bboxes.push(mk_bbox(320.0, y, 170.0, 14.0));
            y += 14.0;
        }
        let pr = run(bboxes, &geometry);
        let mut leaves = Vec::new();
        collect_leaves(&pr.root, &mut leaves);
        // Expect labels: "1" (top single-col), "2-1" (bottom-left col), "2-2" (bottom-right col).
        assert_eq!(leaves.len(), 3, "expected 3 leaves");
        assert_eq!(leaves[0].label, "1");
        assert_eq!(leaves[1].label, "2-1");
        assert_eq!(leaves[2].label, "2-2");
    }

    // --- 7. Element-index leaves resolve to body indices ----------------------
    #[test]
    fn leaf_element_indices_cover_body_elements() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let bboxes = vec![
            mk_bbox(110.0, 80.0, 380.0, 14.0),
            mk_bbox(110.0, 100.0, 380.0, 14.0),
            mk_bbox(110.0, 120.0, 380.0, 14.0),
        ];
        let pr = run(bboxes, &geometry);
        let mut leaves = Vec::new();
        collect_leaves(&pr.root, &mut leaves);
        let mut idxs: Vec<u32> = leaves
            .iter()
            .flat_map(|l| l.element_indices.clone())
            .collect();
        idxs.sort();
        assert_eq!(idxs, vec![0, 1, 2]);
    }

    // --- 8. Empty body box → empty leaf ---------------------------------------
    #[test]
    fn empty_body_yields_single_empty_leaf() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let pr = run(vec![], &geometry);
        assert_eq!(count_leaves(&pr.root), 1);
        let mut leaves = Vec::new();
        collect_leaves(&pr.root, &mut leaves);
        assert!(leaves[0].element_indices.is_empty());
    }

    // --- 9. Determinism: same input → byte-identical JSON ---------------------
    #[test]
    fn determinism_byte_identical_json() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![300.0]);
        let mut bboxes = Vec::new();
        let mut y = 80.0;
        while y + 14.0 <= 700.0 {
            bboxes.push(mk_bbox(110.0, y, 170.0, 14.0));
            bboxes.push(mk_bbox(320.0, y, 170.0, 14.0));
            y += 14.0;
        }
        let a = serde_json::to_string(&run(bboxes.clone(), &geometry).root).unwrap();
        let b = serde_json::to_string(&run(bboxes, &geometry).root).unwrap();
        assert_eq!(a, b);
    }

    // --- 10. Body box uses doc_footer_y, not per_page_footer_y ----------------
    #[test]
    fn body_box_uses_doc_footer_y_per_marcus_2026_05_06() {
        // Set doc_footer_y above per_page_footer_y to verify we're using
        // doc_footer_y. (per_page_footer_y is irrelevant in body_box_for_page
        // — this test pins the contract.)
        let mut geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        geometry.per_page_footer_y = vec![Some(550.0)];
        let body = body_box_for_page(&geometry);
        assert_eq!(body.y1, 720.0, "body box y1 must use doc_footer_y");
    }
}
