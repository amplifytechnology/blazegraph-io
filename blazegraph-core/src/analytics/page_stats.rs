// Per-region signatures + per-page primitives. Block 04 of the document
// analytics flow. Reshaped 2026-05-06 to consume Region trees from
// `RegionStats` (instead of detecting bands from Tika output) — the metric
// algorithms (density, X/Y peaks, visual-line dedup, italic/bold/normal,
// heatmap fit) survived verbatim from the 2026-05-02 spec; only the
// structural unit changed.
//
// Algorithm canonical: `scripts/band_signatures.py`. Empirically validated
// on the 6-PDF corpus during prototyping. See
// `docs/P2/core/handoffs/2026-05-06-block04-page-stats-region-aware.md`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::analytics::page_roles::PageRoleKind;
use crate::analytics::region::{PageRegions, Region, RegionBox};
use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::{BoundingBox, PdfTextElement};

// ---------------------------------------------------------------------------
// Output type shape
// ---------------------------------------------------------------------------

/// Per-page + per-region descriptive primitives for the document.
///
/// `pages` carries one entry per page in observation order (matches
/// `PageStatsBuilder` page slots, which is also reading order across the
/// document). `regions` carries one entry per leaf in each page's Region
/// tree, page-major then depth-first within the page (matches reading
/// order). Pages without a `RegionStats` entry contribute a `PageSignature`
/// but no `RegionSignature`s.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageStats {
    /// One entry per page in the analysis window.
    pub pages: Vec<PageSignature>,
    /// Per-region signatures across all analyzed pages. Each region carries
    /// its `page_number` and `region_label` so consumers can group by page
    /// and resolve back to the matching leaf in `RegionStats.per_page[…].root`.
    pub regions: Vec<RegionSignature>,
    /// Inclusive 1-indexed body extent: pages `[body_start_page,
    /// body_end_page]` are classified as `Body`. `0` in either field means
    /// "no body detected" (e.g., empty document, classifier didn't run, or
    /// every page failed the body test). Populated by the page-roles
    /// classifier (`analytics::page_roles`) at the tail of
    /// `AnalysisBuilder::finalize`.
    #[serde(default)]
    pub body_start_page: u32,
    #[serde(default)]
    pub body_end_page: u32,
}

/// Whole-page primitives. Composition is computed across all non-rotated
/// elements on the page (NOT body-filtered) so cover pages — where content
/// sits above `header_y` or outside `left_x`/`right_x` — still register
/// their content. `heatmap_fit` and `n_peaks_y` are restricted to the body
/// region.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageSignature {
    pub page_number: u32,

    /// Mutually exclusive whole-page token counts. Sum to `n_tokens`.
    /// `italic` includes bold-italic; `bold` is bold-AND-NOT-italic;
    /// `normal` is neither. Counted across whitespace-split words.
    pub n_tokens: u32,
    pub italic_tokens: u32,
    pub bold_tokens: u32,
    pub normal_tokens: u32,

    /// Body-likeness score in [0, 1]. Page bbox-coverage projected onto the
    /// `GeometryStats.heatmap` density grid, restricted to the body region
    /// (header_y..doc_footer_y, left_x..right_x). `~1.0` means the page
    /// covers the same body region as the average page; near 0 means
    /// cover / blank / sparse.
    pub heatmap_fit: f32,

    /// Page-level Y-peak count from bbox-presence projection over the body
    /// region. One peak per visual row.
    pub n_peaks_y: u32,
    /// Coefficient of variation of inter-Y-peak gaps. Lower = more regular
    /// vertical spacing. 0.0 when fewer than 3 peaks.
    pub y_peak_cv: f32,

    /// Page role assigned by `analytics::page_roles`. `None` until the
    /// classifier runs (or if the classifier was disabled). Downstream
    /// rules filter via this — e.g., section detection skips pages whose
    /// role is `NonBody`.
    #[serde(default)]
    pub role: Option<PageRoleKind>,
}

/// Per-region signature. The unit downstream classifiers reason about.
/// Computed for each leaf of the Region tree from `RegionStats.per_page[p]`.
/// Interior nodes do not get signatures.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegionSignature {
    pub page_number: u32,
    /// Reading-order path from the Region tree leaf, e.g. "1", "2-1",
    /// "3-2-1". Resolves to the matching leaf in
    /// `RegionStats.per_page[…].root`.
    pub region_label: String,

    // ── Geometry (from the Region tree leaf's box) ─────────────
    pub x_left: f32,
    pub x_right: f32,
    pub y_top: f32,
    pub y_bottom: f32,
    /// `(x_right - x_left) × (y_bottom - y_top)`. Floor-clamped to 1.0.
    pub area: f32,

    // ── Line structure ─────────────────────────────────────────
    /// Distinct y-rows the leaf's elements occupy (raw y_top values
    /// dedup'd by the same baseline).
    pub n_lines: u32,
    /// Lines after Y-baseline dedup (sub/superscripts collapsed).
    pub n_visual_lines: u32,
    /// Median Y-extent (in pt) of input elements in the leaf. Used as the
    /// per-region reference for the visual-line tolerance.
    pub median_line_height: f32,

    // ── Token counts (mutually exclusive; sum to `n_tokens`) ───
    pub n_tokens: u32,
    pub italic_tokens: u32,
    pub bold_tokens: u32,
    pub normal_tokens: u32,

    // ── Density (font-size invariant fill ratio) ───────────────
    /// Estimated ink-bearing area: Σ per-token `chars × font_size² × glyph_width_ratio`.
    pub glyph_area: f32,
    /// `glyph_area / area`. Body prose ≈ 0.4-0.6 typically.
    pub density_raw: f32,
    /// Per-document normalization: `density_raw / max(density_raw across all
    /// regions in the document)`. Lands in [0, 1].
    pub density: f32,

    // ── Multi-column structure within the region ───────────────
    /// X-projection sustained-peak count. ≥ 2 indicates multi-column
    /// content (table or list); 1 indicates flowing prose.
    pub n_peaks: u32,

    // ── Y-spacing regularity ───────────────────────────────────
    /// Y-projection presence-based peak count. One peak per visual row in
    /// the leaf's Y-extent.
    pub n_peaks_y: u32,
    /// Coefficient of variation of inter-Y-peak gaps. 0.0 when < 3 peaks.
    pub y_peak_cv: f32,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tunable parameters. Defaults are taken verbatim from the validated
/// prototype (`scripts/band_signatures.py`).
#[derive(Debug, Clone)]
pub struct PageStatsConfig {
    /// X-peak detection: peak threshold as fraction of max projection.
    /// A run is a "peak" when projection ≥ ratio × max. Default: 0.50.
    pub peak_x_high_ratio: f32,
    /// X-peak detection: minimum sustained run length in pt to count as
    /// a peak (filters single-character noise). Default: 3.
    pub peak_x_min_width: u32,
    /// Y-peak detection minimum run length in pt. Default: 1pt — catch
    /// every visual row including small-font footnotes.
    pub peak_y_min_height: u32,
    /// Glyph aspect ratio (width / height) for glyph-area estimation.
    /// Default: 0.5. Per-document normalization makes the exact value
    /// mostly irrelevant.
    pub glyph_width_ratio: f32,
    /// Visual-line dedup tolerance: lines with `y_center` within
    /// `max(absolute_floor, fraction × median_line_height)` cluster into
    /// one visual line.
    pub visual_line_tolerance_floor: f32,
    pub visual_line_tolerance_fraction: f32,
    /// Minimum tokens required for a span to count toward composition.
    /// Default: 1 (any non-empty span).
    pub min_span_tokens: u32,
}

impl Default for PageStatsConfig {
    fn default() -> Self {
        Self {
            peak_x_high_ratio: 0.50,
            peak_x_min_width: 3,
            peak_y_min_height: 1,
            glyph_width_ratio: 0.5,
            visual_line_tolerance_floor: 4.0,
            visual_line_tolerance_fraction: 0.5,
            min_span_tokens: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ObservedSpan {
    global_idx: u32,
    bbox: BoundingBox,
    font_size: f32,
    is_italic: bool,
    is_bold: bool,
    text: String,
}

#[derive(Debug, Default)]
struct PageObservation {
    page_number: u32,
    /// All non-rotated elements in observation order. Each carries its
    /// global index so we can match against
    /// `RegionStats.per_page[p].body_element_indices` during finalize.
    elements: Vec<ObservedSpan>,
}

/// Builder for per-page + per-region descriptive primitives.
#[derive(Debug, Default)]
pub struct PageStatsBuilder {
    config: PageStatsConfig,
    pages: Vec<PageObservation>,
    /// Document-wide element counter. Increments on every observe call
    /// regardless of filtering, so leaves in `RegionStats` resolve back to
    /// elements in the same global stream.
    next_global_idx: u32,
}

impl PageStatsBuilder {
    /// Construct a builder with explicit config. The `AnalysisBuilder` in
    /// `builder.rs` uses `PageStatsBuilder::default()`.
    pub fn new(config: PageStatsConfig) -> Self {
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
            elements: Vec::new(),
        });
        self.pages.len() - 1
    }
}

impl Statistic for PageStatsBuilder {
    type Output = PageStats;
    const NAME: &'static str = "page_stats";

    fn observe(&mut self, element: &PdfTextElement) {
        let global_idx = self.next_global_idx;
        self.next_global_idx = self.next_global_idx.saturating_add(1);

        if element.rotation() != 0 {
            return;
        }

        let style = &element.style_info;
        let span = ObservedSpan {
            global_idx,
            bbox: element.bounding_box().clone(),
            font_size: style.font_size,
            is_italic: is_italic(style),
            is_bold: is_bold(style),
            text: element.text.clone(),
        };

        let page_number = element.page_number();
        let idx = self.page_slot(page_number);
        self.pages[idx].elements.push(span);
    }

    fn finalize(self, ctx: &FinalizationContext<'_>) -> Self::Output {
        let geometry = match ctx.geometry {
            Some(g) => g,
            None => return PageStats::default(),
        };
        let region_stats = match ctx.region {
            Some(r) => r,
            None => return PageStats::default(),
        };

        let body_box = RegionBox {
            x0: geometry.left_x,
            y0: geometry.header_y,
            x1: geometry.right_x,
            y1: geometry.doc_footer_y,
        };

        // Per-page index: page_number → PageRegions reference. Used to find
        // the Region tree for each PageObservation.
        let region_by_page: HashMap<u32, &PageRegions> = region_stats
            .per_page
            .iter()
            .map(|p| (p.page_number, p))
            .collect();

        let mut pages: Vec<PageSignature> = Vec::with_capacity(self.pages.len());
        let mut regions: Vec<RegionSignature> = Vec::new();

        for page in &self.pages {
            // Per-page composition — across ALL non-rotated elements on the
            // page (no body filter). Cover pages have content above header_y;
            // we still want to count it.
            let composition = compute_composition(&page.elements, self.config.min_span_tokens);

            // Per-page heatmap_fit + Y-peak structure restrict to body.
            let heatmap_fit = compute_heatmap_fit(&page.elements, &geometry.heatmap, &body_box);
            let (n_peaks_y, y_peak_cv) = page_y_peaks(&page.elements, &body_box);

            pages.push(PageSignature {
                page_number: page.page_number,
                n_tokens: composition.n_tokens,
                italic_tokens: composition.italic_tokens,
                bold_tokens: composition.bold_tokens,
                normal_tokens: composition.normal_tokens,
                heatmap_fit,
                n_peaks_y,
                y_peak_cv,
                role: None,
            });

            // Per-region signatures: only if RegionStats has an entry for
            // this page (it always does when finalize is reached through
            // AnalysisBuilder, but tests may skip).
            let Some(page_regions) = region_by_page.get(&page.page_number) else {
                continue;
            };

            // Map global_idx → ObservedSpan index, built once per page so
            // the per-leaf lookup is O(1).
            let by_global: HashMap<u32, usize> = page
                .elements
                .iter()
                .enumerate()
                .map(|(i, s)| (s.global_idx, i))
                .collect();

            walk_leaves(&page_regions.root, &mut |leaf| {
                let signature = build_region_signature(
                    leaf,
                    page.page_number,
                    &page.elements,
                    &by_global,
                    &page_regions.body_element_indices,
                    &self.config,
                );
                regions.push(signature);
            });
        }

        // Per-document density normalization.
        let max_density_raw = regions
            .iter()
            .map(|r| r.density_raw)
            .fold(0.0_f32, f32::max);
        if max_density_raw > 0.0 {
            for r in regions.iter_mut() {
                r.density = (r.density_raw / max_density_raw).min(1.0);
            }
        }

        PageStats {
            pages,
            regions,
            body_start_page: 0,
            body_end_page: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Font-flag predicates (mirror the pipeline pattern from
// rules/section_detection_v2.rs::is_bold and the prototype's
// _is_italic_span).
// ---------------------------------------------------------------------------

fn is_bold(style: &crate::types::FontClass) -> bool {
    let weight = style.font_weight.to_lowercase();
    if weight.contains("bold") {
        return true;
    }
    let family = style.font_family.to_lowercase();
    family.contains("bold") || family.contains("medi") || family.contains("bx")
}

fn is_italic(style: &crate::types::FontClass) -> bool {
    let s = style.font_style.to_lowercase();
    if s.contains("italic") || s.contains("oblique") {
        return true;
    }
    let family = style.font_family.to_lowercase();
    family.contains("italic") || family.contains("oblique") || family.ends_with("-ital")
}

// ---------------------------------------------------------------------------
// Composition (whole-page or per-leaf)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Composition {
    n_tokens: u32,
    italic_tokens: u32,
    bold_tokens: u32,
    normal_tokens: u32,
}

fn count_tokens(text: &str) -> u32 {
    text.split_whitespace().filter(|t| !t.is_empty()).count() as u32
}

fn count_visible_chars(text: &str) -> u32 {
    text.chars().filter(|c| !c.is_whitespace()).count() as u32
}

/// Mutually-exclusive italic/bold/normal partition. `italic` includes
/// bold-italic by deliberate convention; `bold` is bold AND NOT italic;
/// `normal` is neither.
fn compute_composition(spans: &[ObservedSpan], min_span_tokens: u32) -> Composition {
    let mut c = Composition::default();
    for s in spans {
        let tokens = count_tokens(&s.text);
        if tokens < min_span_tokens {
            continue;
        }
        c.n_tokens += tokens;
        if s.is_italic {
            c.italic_tokens += tokens;
        } else if s.is_bold {
            c.bold_tokens += tokens;
        }
    }
    c.normal_tokens = c.n_tokens - c.italic_tokens - c.bold_tokens;
    c
}

// ---------------------------------------------------------------------------
// Heatmap fit (per-page)
// ---------------------------------------------------------------------------

/// Build a 0/1 page coverage mask at heatmap.cell_size resolution, restrict
/// to the body region, return Σ heatmap × mask / Σ heatmap.
fn compute_heatmap_fit(
    spans: &[ObservedSpan],
    heatmap: &crate::analytics::geometry::DensityGrid,
    body: &RegionBox,
) -> f32 {
    if heatmap.cell_size == 0 || heatmap.cells.is_empty() || heatmap.cols == 0 || heatmap.rows == 0
    {
        return 0.0;
    }
    let cs = heatmap.cell_size as i32;
    let cols = heatmap.cols as i32;
    let rows = heatmap.rows as i32;

    // 0/1 mask at cell granularity.
    let mut mask = vec![false; (rows as usize) * (cols as usize)];
    for s in spans {
        let x0 = (s.bbox.x / cs as f32).floor() as i32;
        let x1 = ((s.bbox.x + s.bbox.width) / cs as f32).ceil() as i32;
        let y0 = (s.bbox.y / cs as f32).floor() as i32;
        let y1 = ((s.bbox.y + s.bbox.height) / cs as f32).ceil() as i32;
        let xa = x0.clamp(0, cols);
        let xb = x1.clamp(0, cols);
        let ya = y0.clamp(0, rows);
        let yb = y1.clamp(0, rows);
        if xb <= xa || yb <= ya {
            continue;
        }
        for r in ya..yb {
            let row_off = (r as usize) * (cols as usize);
            for c in xa..xb {
                mask[row_off + c as usize] = true;
            }
        }
    }

    // Body cell window.
    let body_y0 = (body.y0 / cs as f32).floor() as i32;
    let body_y1 = (body.y1 / cs as f32).ceil() as i32;
    let body_x0 = (body.x0 / cs as f32).floor() as i32;
    let body_x1 = (body.x1 / cs as f32).ceil() as i32;
    let ya = body_y0.clamp(0, rows);
    let yb = body_y1.clamp(0, rows);
    let xa = body_x0.clamp(0, cols);
    let xb = body_x1.clamp(0, cols);
    if yb <= ya || xb <= xa {
        return 0.0;
    }

    let mut total: f64 = 0.0;
    let mut covered: f64 = 0.0;
    for r in ya..yb {
        let row_off = (r as usize) * (cols as usize);
        for c in xa..xb {
            let v = heatmap.cells[row_off + c as usize] as f64;
            total += v;
            if mask[row_off + c as usize] {
                covered += v;
            }
        }
    }
    if total <= 0.0 {
        return 0.0;
    }
    (covered / total) as f32
}

// ---------------------------------------------------------------------------
// Y-peaks (page-level + per-region)
// ---------------------------------------------------------------------------

/// Page-level Y-peak count + cv over the body region. Mirrors `measure_y_peaks`
/// in the prototype but applied to the body Y-extent on the page.
fn page_y_peaks(spans: &[ObservedSpan], body: &RegionBox) -> (u32, f32) {
    let y_top = body.y0;
    let y_bottom = body.y1;
    measure_y_peaks_in(spans.iter().map(|s| &s.bbox), y_top, y_bottom)
}

/// Presence-based Y-peak count + CV of inter-peak gaps. Each contiguous
/// run of "any bbox covers this 1pt Y-row" is one peak.
fn measure_y_peaks_in<'a>(
    bboxes: impl Iterator<Item = &'a BoundingBox>,
    y_top: f32,
    y_bottom: f32,
) -> (u32, f32) {
    let height = (y_bottom - y_top).ceil() as i32;
    if height <= 0 {
        return (0, 0.0);
    }
    let h = height as usize;
    let mut presence = vec![false; h];
    for b in bboxes {
        let a = ((b.y - y_top).floor() as i32).max(0);
        let bb = ((b.y + b.height - y_top).ceil() as i32).min(height);
        if bb <= a {
            continue;
        }
        presence[(a as usize)..(bb as usize)].fill(true);
    }

    let mut centers: Vec<f32> = Vec::new();
    let mut in_peak = false;
    let mut start: usize = 0;
    for (i, &p) in presence.iter().enumerate() {
        if p && !in_peak {
            in_peak = true;
            start = i;
        } else if !p && in_peak {
            in_peak = false;
            centers.push((start as f32 + (i as f32 - 1.0)) / 2.0);
        }
    }
    if in_peak {
        centers.push((start as f32 + (h as f32 - 1.0)) / 2.0);
    }

    let n = centers.len() as u32;
    if centers.len() < 3 {
        return (n, 0.0);
    }
    let gaps: Vec<f32> = centers.windows(2).map(|w| w[1] - w[0]).collect();
    let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
    if mean <= 0.0 {
        return (n, 0.0);
    }
    let var = gaps.iter().map(|g| (g - mean).powi(2)).sum::<f32>() / gaps.len() as f32;
    (n, var.sqrt() / mean)
}

// ---------------------------------------------------------------------------
// X-peaks (per-region)
// ---------------------------------------------------------------------------

/// Sustained-peak count via 1pt X-projection across `[x_left, x_right)`.
fn count_x_peaks(
    spans: &[&ObservedSpan],
    x_left: f32,
    x_right: f32,
    high_ratio: f32,
    min_peak_width: u32,
) -> u32 {
    let width = (x_right - x_left).ceil() as i32;
    if width <= 0 {
        return 0;
    }
    let w = width as usize;
    let mut proj = vec![0u32; w];
    for s in spans {
        let a = ((s.bbox.x - x_left).floor() as i32).max(0);
        let b = ((s.bbox.x + s.bbox.width - x_left).ceil() as i32).min(width);
        if b <= a {
            continue;
        }
        for v in &mut proj[(a as usize)..(b as usize)] {
            *v += 1;
        }
    }
    let peak_val = proj.iter().copied().max().unwrap_or(0);
    if peak_val == 0 {
        return 0;
    }
    let threshold = high_ratio * peak_val as f32;
    let mut n_peaks = 0u32;
    let mut in_peak = false;
    let mut start: usize = 0;
    for (i, &v) in proj.iter().enumerate() {
        if (v as f32) >= threshold {
            if !in_peak {
                in_peak = true;
                start = i;
            }
        } else if in_peak {
            in_peak = false;
            if (i - start) as u32 >= min_peak_width {
                n_peaks += 1;
            }
        }
    }
    if in_peak && (w - start) as u32 >= min_peak_width {
        n_peaks += 1;
    }
    n_peaks
}

// ---------------------------------------------------------------------------
// Visual lines (per-region)
// ---------------------------------------------------------------------------

/// Cluster `y_center` values within `max(floor, fraction × mlh)` and return
/// cluster count.
fn count_visual_lines(
    spans: &[&ObservedSpan],
    median_line_height: f32,
    config: &PageStatsConfig,
) -> u32 {
    if spans.is_empty() {
        return 0;
    }
    let tolerance = config
        .visual_line_tolerance_floor
        .max(config.visual_line_tolerance_fraction * median_line_height);
    let mut centers: Vec<f32> = spans
        .iter()
        .map(|s| s.bbox.y + s.bbox.height / 2.0)
        .collect();
    centers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut n = 1u32;
    let mut last = centers[0];
    for &y in &centers[1..] {
        if y - last > tolerance {
            n += 1;
        }
        last = y;
    }
    n
}

/// Distinct y-baselines (y_top values dedup'd by exact equality, via bit
/// pattern). Test 9 expects 4 distinct y_tops → 4 lines, even if very close.
fn count_raw_lines(spans: &[&ObservedSpan]) -> u32 {
    use std::collections::HashSet;
    let mut seen: HashSet<u32> = HashSet::new();
    for s in spans {
        seen.insert(s.bbox.y.to_bits());
    }
    seen.len() as u32
}

fn median_height(spans: &[&ObservedSpan]) -> f32 {
    if spans.is_empty() {
        return 0.0;
    }
    let mut hs: Vec<f32> = spans.iter().map(|s| s.bbox.height).collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    hs[hs.len() / 2]
}

// ---------------------------------------------------------------------------
// Per-region signature build
// ---------------------------------------------------------------------------

fn build_region_signature(
    leaf: &Region,
    page_number: u32,
    page_spans: &[ObservedSpan],
    by_global: &HashMap<u32, usize>,
    body_element_indices: &[u32],
    config: &PageStatsConfig,
) -> RegionSignature {
    // Resolve leaf-local indices → global indices → ObservedSpan refs.
    let leaf_spans: Vec<&ObservedSpan> = leaf
        .element_indices
        .iter()
        .filter_map(|&local| body_element_indices.get(local as usize).copied())
        .filter_map(|global| by_global.get(&global).map(|&i| &page_spans[i]))
        .collect();

    let bx = leaf.r#box;
    let area = ((bx.x1 - bx.x0) * (bx.y1 - bx.y0)).max(1.0);

    let mlh = median_height(&leaf_spans);
    let n_lines = count_raw_lines(&leaf_spans);
    let n_visual_lines = count_visual_lines(&leaf_spans, mlh, config);

    // Composition + glyph area.
    let mut n_tokens = 0u32;
    let mut italic_tokens = 0u32;
    let mut bold_tokens = 0u32;
    let mut glyph_area = 0.0_f32;
    for s in &leaf_spans {
        let tokens = count_tokens(&s.text);
        if tokens >= config.min_span_tokens && tokens > 0 {
            n_tokens += tokens;
            if s.is_italic {
                italic_tokens += tokens;
            } else if s.is_bold {
                bold_tokens += tokens;
            }
        }
        // Density: glyph_area accumulates regardless of token gating —
        // a span with one whitespace-only token still has glyphs. (The
        // prototype runs unconditionally; we do too.)
        let n_chars = count_visible_chars(&s.text) as f32;
        glyph_area += n_chars * s.font_size * s.font_size * config.glyph_width_ratio;
    }
    let normal_tokens = n_tokens - italic_tokens - bold_tokens;
    let density_raw = glyph_area / area;

    // Peaks within the leaf box.
    let n_peaks = count_x_peaks(
        &leaf_spans,
        bx.x0,
        bx.x1,
        config.peak_x_high_ratio,
        config.peak_x_min_width,
    );
    let (n_peaks_y, y_peak_cv) =
        measure_y_peaks_in(leaf_spans.iter().map(|s| &s.bbox), bx.y0, bx.y1);

    RegionSignature {
        page_number,
        region_label: leaf.label.clone(),
        x_left: bx.x0,
        x_right: bx.x1,
        y_top: bx.y0,
        y_bottom: bx.y1,
        area,
        n_lines,
        n_visual_lines,
        median_line_height: mlh,
        n_tokens,
        italic_tokens,
        bold_tokens,
        normal_tokens,
        glyph_area,
        density_raw,
        density: 0.0,
        n_peaks,
        n_peaks_y,
        y_peak_cv,
    }
}

// ---------------------------------------------------------------------------
// Tree walk (depth-first; reading order matches Region.label assignment).
// ---------------------------------------------------------------------------

fn walk_leaves<F: FnMut(&Region)>(region: &Region, f: &mut F) {
    if region.children.is_empty() {
        f(region);
    } else {
        for c in &region.children {
            walk_leaves(c, f);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::geometry::{ColumnLayout, DensityGrid, GeometryStats};
    use crate::analytics::region::{
        CutAxis, PageRegionDiagnostic, PageRegions, Region, RegionBox, RegionStats,
        RegionStatsBuilder,
    };
    use crate::types::{FontClass, Placement};

    // -- Element + geometry construction ----------------------------------

    #[allow(clippy::too_many_arguments)]
    fn mk_element(
        page: u32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        font_size: f32,
        text: &str,
        italic: bool,
        bold: bool,
    ) -> PdfTextElement {
        let style = if italic && bold {
            ("italic", "bold", "Times-BoldItalic")
        } else if italic {
            ("italic", "normal", "Times-Italic")
        } else if bold {
            ("normal", "bold", "Times-Bold")
        } else {
            ("normal", "normal", "Times")
        };
        PdfTextElement {
            text: text.to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: style.2.to_string(),
                font_size,
                font_style: style.0.to_string(),
                font_weight: style.1.to_string(),
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
            token_count: count_tokens(text) as usize,
            raw_tags: vec![],
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

    /// Add a uniform density-grid heatmap covering the full body, with a
    /// constant cell value across all body cells. Outside-body cells stay 0.
    fn with_uniform_body_heatmap(geometry: &mut GeometryStats, cell_size: u32, value: u16) {
        let cols = ((geometry.right_x / cell_size as f32).ceil() as u32).max(1);
        let rows = ((geometry.doc_footer_y / cell_size as f32).ceil() as u32).max(1);
        let mut cells = vec![0u16; (rows * cols) as usize];
        let r0 = (geometry.header_y / cell_size as f32).floor() as u32;
        let r1 = (geometry.doc_footer_y / cell_size as f32).ceil() as u32;
        let c0 = (geometry.left_x / cell_size as f32).floor() as u32;
        let c1 = (geometry.right_x / cell_size as f32).ceil() as u32;
        for r in r0..r1.min(rows) {
            for c in c0..c1.min(cols) {
                cells[(r * cols + c) as usize] = value;
            }
        }
        geometry.heatmap = DensityGrid {
            cell_size,
            cols,
            rows,
            cells,
        };
    }

    /// Run RegionStatsBuilder + PageStatsBuilder on the elements and return
    /// the finalized PageStats. Mirrors AnalysisBuilder::finalize for the
    /// stats this block depends on.
    fn run_pipeline(elements: &[PdfTextElement], geometry: &GeometryStats) -> PageStats {
        let mut region_b = RegionStatsBuilder::default();
        let mut page_b = PageStatsBuilder::default();
        for e in elements {
            region_b.observe(e);
            page_b.observe(e);
        }
        let region_ctx = FinalizationContext {
            font: None,
            geometry: Some(geometry),
            region: None,
        };
        let region = region_b.finalize(&region_ctx);
        let page_ctx = FinalizationContext {
            font: None,
            geometry: Some(geometry),
            region: Some(&region),
        };
        page_b.finalize(&page_ctx)
    }

    /// Construct a synthetic single-leaf PageRegions covering an entire body
    /// box, with element_indices `0..n`. Used by metric-focused unit tests.
    fn synth_single_leaf_page_regions(
        page_number: u32,
        bx: RegionBox,
        n_body: usize,
    ) -> PageRegions {
        let leaf = Region {
            r#box: bx,
            axis: None,
            cut_coords: vec![],
            children: vec![],
            label: "1".to_string(),
            element_indices: (0..n_body as u32).collect(),
        };
        PageRegions {
            page_number,
            body_box: bx,
            median_line_height: 12.0,
            body_element_indices: (0..n_body as u32).collect(),
            root: leaf,
            diagnostic: PageRegionDiagnostic::default(),
        }
    }

    /// Run page_stats finalize against a hand-built RegionStats. Used when
    /// the test wants direct control over the leaf shape rather than going
    /// through XY-cut.
    fn finalize_with_synth_regions(
        elements: &[PdfTextElement],
        geometry: &GeometryStats,
        region: RegionStats,
    ) -> PageStats {
        let mut page_b = PageStatsBuilder::default();
        for e in elements {
            page_b.observe(e);
        }
        let ctx = FinalizationContext {
            font: None,
            geometry: Some(geometry),
            region: Some(&region),
        };
        page_b.finalize(&ctx)
    }

    // -- 1. Single-leaf body --------------------------------------------------

    #[test]
    fn single_leaf_body_one_region() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let mut elements = Vec::new();
        let mut y = 80.0;
        while y + 14.0 <= 700.0 {
            elements.push(mk_element(
                1,
                110.0,
                y,
                380.0,
                14.0,
                10.0,
                "lorem ipsum dolor sit amet",
                false,
                false,
            ));
            y += 14.0;
        }
        let stats = run_pipeline(&elements, &geometry);
        assert_eq!(stats.regions.len(), 1);
        assert_eq!(stats.regions[0].region_label, "1");
        assert!(stats.regions[0].n_lines > 0);
        assert!(stats.regions[0].density_raw > 0.0);
        assert_eq!(stats.pages.len(), 1);
    }

    // -- 2. Two-section h-cut -------------------------------------------------

    #[test]
    fn two_section_h_cut_two_regions() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let mut elements = Vec::new();
        let mut y = 80.0;
        while y + 14.0 <= 300.0 {
            elements.push(mk_element(
                1,
                110.0,
                y,
                380.0,
                14.0,
                10.0,
                "section one body",
                false,
                false,
            ));
            y += 14.0;
        }
        y = 350.0;
        while y + 14.0 <= 700.0 {
            elements.push(mk_element(
                1,
                110.0,
                y,
                380.0,
                14.0,
                10.0,
                "section two body",
                false,
                false,
            ));
            y += 14.0;
        }
        let stats = run_pipeline(&elements, &geometry);
        assert_eq!(stats.regions.len(), 2);
        assert_eq!(stats.regions[0].region_label, "1");
        assert_eq!(stats.regions[1].region_label, "2");
        assert!(stats.regions[0].density_raw > 0.0);
        assert!(stats.regions[1].density_raw > 0.0);
    }

    // -- 3. Two-column v-cut --------------------------------------------------

    #[test]
    fn two_column_v_cut_two_regions() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![300.0]);
        let mut elements = Vec::new();
        let mut y = 80.0;
        while y + 14.0 <= 700.0 {
            elements.push(mk_element(
                1,
                110.0,
                y,
                170.0,
                14.0,
                10.0,
                "left column line",
                false,
                false,
            ));
            elements.push(mk_element(
                1,
                320.0,
                y,
                170.0,
                14.0,
                10.0,
                "right column line",
                false,
                false,
            ));
            y += 14.0;
        }
        let stats = run_pipeline(&elements, &geometry);
        assert_eq!(stats.regions.len(), 2);
        assert_eq!(stats.regions[0].region_label, "1");
        assert_eq!(stats.regions[1].region_label, "2");
    }

    // -- 4. Density font-size invariance --------------------------------------

    #[test]
    fn density_is_font_size_invariant() {
        // Two leaves with equivalent visual fill but different font sizes.
        // Leaf A: 10 chars × 10pt covering ~50pt × 10pt at high density.
        // Leaf B: 5 chars × 20pt covering same ~50pt × ~20pt — same visual
        // ink coverage, double the font size. Density should match within
        // 5% (boundary effects from leaf-box vs element-extent).
        let bx_a = RegionBox {
            x0: 100.0,
            y0: 100.0,
            x1: 150.0,
            y1: 110.0,
        };
        let bx_b = RegionBox {
            x0: 100.0,
            y0: 200.0,
            x1: 150.0,
            y1: 220.0,
        };

        // Build elements that fill these boxes exactly.
        let elements = vec![
            // Leaf A: 10 chars at 10pt font, bbox 100..150 × 100..110
            mk_element(
                1,
                100.0,
                100.0,
                50.0,
                10.0,
                10.0,
                "0123456789",
                false,
                false,
            ),
            // Leaf B: 5 chars at 20pt font, bbox 100..150 × 200..220
            mk_element(1, 100.0, 200.0, 50.0, 20.0, 20.0, "01234", false, false),
        ];

        // Synthetic two-leaf RegionStats: root with H-axis, two leaves.
        let root = Region {
            r#box: RegionBox {
                x0: 100.0,
                y0: 100.0,
                x1: 150.0,
                y1: 220.0,
            },
            axis: Some(CutAxis::H),
            cut_coords: vec![150.0],
            children: vec![
                Region {
                    r#box: bx_a,
                    axis: None,
                    cut_coords: vec![],
                    children: vec![],
                    label: "1".to_string(),
                    element_indices: vec![0],
                },
                Region {
                    r#box: bx_b,
                    axis: None,
                    cut_coords: vec![],
                    children: vec![],
                    label: "2".to_string(),
                    element_indices: vec![1],
                },
            ],
            label: String::new(),
            element_indices: vec![],
        };
        let region = RegionStats {
            per_page: vec![PageRegions {
                page_number: 1,
                body_box: RegionBox {
                    x0: 100.0,
                    y0: 100.0,
                    x1: 150.0,
                    y1: 220.0,
                },
                median_line_height: 14.0,
                body_element_indices: vec![0, 1],
                root,
                diagnostic: PageRegionDiagnostic::default(),
            }],
            source_pages: 1,
        };

        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let stats = finalize_with_synth_regions(&elements, &geometry, region);
        assert_eq!(stats.regions.len(), 2);
        let d_a = stats.regions[0].density_raw;
        let d_b = stats.regions[1].density_raw;
        let rel = (d_a - d_b).abs() / d_a.max(d_b);
        assert!(
            rel <= 0.05,
            "density_raw not font-size invariant: A={d_a} B={d_b} rel={rel}"
        );
    }

    // -- 5. Density per-document normalization --------------------------------

    #[test]
    fn density_normalization_high_eq_one_low_eq_half() {
        // Two leaves, one with 2× the raw fill of the other.
        // Leaf A (high): 20 chars × 10pt in 50×10 box → glyph_area=1000, density_raw=2.0
        // Leaf B (low): 10 chars × 10pt in 50×10 box → glyph_area=500, density_raw=1.0
        // After normalization: high=1.0, low=0.5.
        let bx_a = RegionBox {
            x0: 100.0,
            y0: 100.0,
            x1: 150.0,
            y1: 110.0,
        };
        let bx_b = RegionBox {
            x0: 100.0,
            y0: 200.0,
            x1: 150.0,
            y1: 210.0,
        };
        let elements = vec![
            mk_element(
                1,
                100.0,
                100.0,
                50.0,
                10.0,
                10.0,
                "01234567890123456789",
                false,
                false,
            ),
            mk_element(
                1,
                100.0,
                200.0,
                50.0,
                10.0,
                10.0,
                "0123456789",
                false,
                false,
            ),
        ];
        let root = Region {
            r#box: RegionBox {
                x0: 100.0,
                y0: 100.0,
                x1: 150.0,
                y1: 210.0,
            },
            axis: Some(CutAxis::H),
            cut_coords: vec![150.0],
            children: vec![
                Region {
                    r#box: bx_a,
                    axis: None,
                    cut_coords: vec![],
                    children: vec![],
                    label: "1".to_string(),
                    element_indices: vec![0],
                },
                Region {
                    r#box: bx_b,
                    axis: None,
                    cut_coords: vec![],
                    children: vec![],
                    label: "2".to_string(),
                    element_indices: vec![1],
                },
            ],
            label: String::new(),
            element_indices: vec![],
        };
        let region = RegionStats {
            per_page: vec![PageRegions {
                page_number: 1,
                body_box: bx_a,
                median_line_height: 14.0,
                body_element_indices: vec![0, 1],
                root,
                diagnostic: PageRegionDiagnostic::default(),
            }],
            source_pages: 1,
        };
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let stats = finalize_with_synth_regions(&elements, &geometry, region);
        let high = stats
            .regions
            .iter()
            .find(|r| r.region_label == "1")
            .unwrap();
        let low = stats
            .regions
            .iter()
            .find(|r| r.region_label == "2")
            .unwrap();
        assert!(
            (high.density - 1.0).abs() < 1e-4,
            "high density = {}",
            high.density
        );
        assert!(
            (low.density - 0.5).abs() < 0.01,
            "low density = {}",
            low.density
        );
    }

    // -- 6. n_peaks: prose vs multi-column ------------------------------------

    #[test]
    fn n_peaks_prose_one_table_three() {
        // Prose leaf: single uniform row of text covering the full width.
        // X-projection has roughly constant max → 1 peak above threshold.
        let bx_prose = RegionBox {
            x0: 100.0,
            y0: 100.0,
            x1: 400.0,
            y1: 200.0,
        };
        let mut prose_elems: Vec<PdfTextElement> = Vec::new();
        for i in 0..5 {
            prose_elems.push(mk_element(
                1,
                100.0,
                100.0 + i as f32 * 14.0,
                300.0,
                14.0,
                10.0,
                "lorem ipsum",
                false,
                false,
            ));
        }
        let region_prose = RegionStats {
            per_page: vec![synth_single_leaf_page_regions(
                1,
                bx_prose,
                prose_elems.len(),
            )],
            source_pages: 1,
        };
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let stats_prose = finalize_with_synth_regions(&prose_elems, &geometry, region_prose);
        assert_eq!(
            stats_prose.regions[0].n_peaks, 1,
            "prose should have 1 peak"
        );

        // Table leaf: 3 cells × 3 rows at consistent X positions.
        let bx_tab = RegionBox {
            x0: 100.0,
            y0: 100.0,
            x1: 400.0,
            y1: 200.0,
        };
        let mut tab_elems: Vec<PdfTextElement> = Vec::new();
        let cells_x = [110.0, 210.0, 310.0];
        for row in 0..3 {
            let y = 110.0 + row as f32 * 16.0;
            for &x in &cells_x {
                tab_elems.push(mk_element(1, x, y, 30.0, 14.0, 10.0, "X", false, false));
            }
        }
        let region_tab = RegionStats {
            per_page: vec![synth_single_leaf_page_regions(1, bx_tab, tab_elems.len())],
            source_pages: 1,
        };
        let stats_tab = finalize_with_synth_regions(&tab_elems, &geometry, region_tab);
        assert_eq!(
            stats_tab.regions[0].n_peaks, 3,
            "3-cell table should have 3 peaks"
        );
    }

    // -- 7. n_peaks_y: presence-counted, not width-weighted ------------------

    #[test]
    fn n_peaks_y_presence_counted() {
        // Two visual rows: one full-width line, one 30%-width line below.
        // Width-weighted projection would bias against the short line and
        // possibly drop it below threshold; presence-counted treats both
        // as 1 visual row each → 2 peaks.
        let bx = RegionBox {
            x0: 100.0,
            y0: 100.0,
            x1: 400.0,
            y1: 200.0,
        };
        let elements = vec![
            mk_element(
                1,
                110.0,
                110.0,
                280.0,
                12.0,
                10.0,
                "full width line",
                false,
                false,
            ),
            mk_element(1, 110.0, 140.0, 84.0, 12.0, 10.0, "short", false, false),
        ];
        let region = RegionStats {
            per_page: vec![synth_single_leaf_page_regions(1, bx, elements.len())],
            source_pages: 1,
        };
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let stats = finalize_with_synth_regions(&elements, &geometry, region);
        assert_eq!(stats.regions[0].n_peaks_y, 2);
    }

    // -- 8. y_peak_cv: regular vs irregular -----------------------------------

    #[test]
    fn y_peak_cv_regular_low_irregular_high() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);

        // Regular: 5 lines at exactly 12pt spacing.
        let bx = RegionBox {
            x0: 100.0,
            y0: 100.0,
            x1: 400.0,
            y1: 200.0,
        };
        let mut regular = Vec::new();
        for i in 0..5 {
            regular.push(mk_element(
                1,
                110.0,
                100.0 + i as f32 * 12.0,
                280.0,
                8.0,
                10.0,
                "x",
                false,
                false,
            ));
        }
        let region_regular = RegionStats {
            per_page: vec![synth_single_leaf_page_regions(1, bx, regular.len())],
            source_pages: 1,
        };
        let stats_regular = finalize_with_synth_regions(&regular, &geometry, region_regular);
        assert!(
            stats_regular.regions[0].y_peak_cv < 0.05,
            "regular y_peak_cv = {}",
            stats_regular.regions[0].y_peak_cv
        );

        // Irregular: 5 lines at varying spacing (10, 12, 14, 16, 18).
        let mut irregular = Vec::new();
        let mut yy = 100.0_f32;
        for &gap in &[10.0_f32, 12.0, 14.0, 16.0, 18.0] {
            irregular.push(mk_element(
                1, 110.0, yy, 280.0, 4.0, 10.0, "x", false, false,
            ));
            yy += gap;
        }
        let bx2 = RegionBox {
            x0: 100.0,
            y0: 100.0,
            x1: 400.0,
            y1: yy + 10.0,
        };
        let region_irreg = RegionStats {
            per_page: vec![synth_single_leaf_page_regions(1, bx2, irregular.len())],
            source_pages: 1,
        };
        let stats_irreg = finalize_with_synth_regions(&irregular, &geometry, region_irreg);
        assert!(
            stats_irreg.regions[0].y_peak_cv > 0.15,
            "irregular y_peak_cv = {}",
            stats_irreg.regions[0].y_peak_cv
        );
    }

    // -- 9. n_visual_lines: sub/superscript collapse --------------------------

    #[test]
    fn n_visual_lines_collapses_sub_superscripts() {
        // Content at y = 100, 100.3, 112, 112.5 → 4 raw lines, 2 visual lines.
        let bx = RegionBox {
            x0: 100.0,
            y0: 90.0,
            x1: 400.0,
            y1: 130.0,
        };
        let elements = vec![
            mk_element(1, 110.0, 100.0, 80.0, 12.0, 10.0, "main", false, false),
            mk_element(1, 195.0, 100.3, 8.0, 8.0, 8.0, "sup", false, false),
            mk_element(1, 110.0, 112.0, 80.0, 12.0, 10.0, "main2", false, false),
            mk_element(1, 195.0, 112.5, 8.0, 8.0, 8.0, "sup2", false, false),
        ];
        let region = RegionStats {
            per_page: vec![synth_single_leaf_page_regions(1, bx, elements.len())],
            source_pages: 1,
        };
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let stats = finalize_with_synth_regions(&elements, &geometry, region);
        assert_eq!(stats.regions[0].n_lines, 4, "raw n_lines should be 4");
        assert_eq!(
            stats.regions[0].n_visual_lines, 2,
            "n_visual_lines should be 2"
        );
    }

    // -- 10. Italic / bold / normal mutual exclusion --------------------------

    #[test]
    fn italic_bold_normal_mutex() {
        // 4 spans: italic, bold, italic-bold, normal — each contributes 2
        // tokens. Italic includes bold-italic; bold is bold-AND-NOT-italic.
        let bx = RegionBox {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 200.0,
        };
        let elements = vec![
            mk_element(1, 110.0, 110.0, 80.0, 12.0, 10.0, "ital one", true, false),
            mk_element(1, 200.0, 110.0, 80.0, 12.0, 10.0, "bold two", false, true),
            mk_element(1, 290.0, 110.0, 80.0, 12.0, 10.0, "ital bold", true, true),
            mk_element(1, 110.0, 130.0, 80.0, 12.0, 10.0, "norm one", false, false),
        ];
        let region = RegionStats {
            per_page: vec![synth_single_leaf_page_regions(1, bx, elements.len())],
            source_pages: 1,
        };
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let stats = finalize_with_synth_regions(&elements, &geometry, region);
        let r = &stats.regions[0];
        assert_eq!(r.n_tokens, 8, "expect 8 tokens");
        assert_eq!(
            r.italic_tokens + r.bold_tokens + r.normal_tokens,
            r.n_tokens
        );
        // Italic span (2) + italic-bold span (2) → italic_tokens = 4.
        assert_eq!(r.italic_tokens, 4);
        // Bold-only span (2) → bold_tokens = 2.
        assert_eq!(r.bold_tokens, 2);
        // Normal span (2) → normal_tokens = 2.
        assert_eq!(r.normal_tokens, 2);
    }

    // -- 11. heatmap_fit ≈ 1.0 for full body coverage -------------------------

    #[test]
    fn heatmap_fit_full_body_one() {
        let mut geometry = mk_geometry(64.0, 720.0, 96.0, 504.0, vec![]);
        with_uniform_body_heatmap(&mut geometry, 8, 100);

        // Cover every body cell with elements.
        let mut elements = Vec::new();
        let mut y = 64.0;
        while y < 720.0 {
            elements.push(mk_element(1, 96.0, y, 408.0, 8.0, 10.0, "x", false, false));
            y += 8.0;
        }
        let stats = run_pipeline(&elements, &geometry);
        let fit = stats.pages[0].heatmap_fit;
        assert!((fit - 1.0).abs() < 0.01, "heatmap_fit = {fit}");
    }

    // -- 12. heatmap_fit == 0 for empty page ----------------------------------

    #[test]
    fn heatmap_fit_empty_zero() {
        let mut geometry = mk_geometry(64.0, 720.0, 96.0, 504.0, vec![]);
        with_uniform_body_heatmap(&mut geometry, 8, 100);
        let elements: Vec<PdfTextElement> = Vec::new();
        let stats = run_pipeline(&elements, &geometry);
        // No pages observed → no PageSignature emitted.
        assert!(stats.pages.is_empty());
        // And the equivalent: a page observed with no body content scores 0.
        let elements = vec![mk_element(
            1, 50.0, 30.0, 40.0, 8.0, 10.0, "header", false, false,
        )];
        let stats = run_pipeline(&elements, &geometry);
        // The single observation is outside body — coverage mask has cells
        // outside body window only; body covered = 0; fit = 0.
        assert_eq!(stats.pages.len(), 1);
        let fit = stats.pages[0].heatmap_fit;
        assert!(fit < 0.01, "heatmap_fit = {fit}, expected near 0");
    }

    // -- 13. heatmap_fit ignores header content -------------------------------

    #[test]
    fn heatmap_fit_ignores_header_content() {
        let mut geometry = mk_geometry(64.0, 720.0, 96.0, 504.0, vec![]);
        with_uniform_body_heatmap(&mut geometry, 8, 100);

        let mut body_only = Vec::new();
        let mut y = 64.0;
        while y < 720.0 {
            body_only.push(mk_element(1, 96.0, y, 408.0, 8.0, 10.0, "b", false, false));
            y += 8.0;
        }
        let stats_body = run_pipeline(&body_only, &geometry);
        let fit_body = stats_body.pages[0].heatmap_fit;

        // Add header content above header_y — should NOT change fit because
        // the heatmap mass outside the body region is zero.
        let mut with_header = body_only.clone();
        with_header.push(mk_element(
            1, 100.0, 30.0, 200.0, 12.0, 10.0, "header", false, false,
        ));
        let stats_h = run_pipeline(&with_header, &geometry);
        let fit_h = stats_h.pages[0].heatmap_fit;
        assert!(
            (fit_body - fit_h).abs() < 1e-3,
            "header content changed fit: body={fit_body} with_header={fit_h}"
        );
    }

    // -- 14. Page-level n_peaks_y + y_peak_cv on regular spacing --------------

    #[test]
    fn page_y_peaks_30_lines_regular() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let mut elements = Vec::new();
        for i in 0..30 {
            // Use 3pt-tall bbox with 12pt step so the empty rows in between
            // are wider than the peak rows, producing 30 distinct presence-runs.
            elements.push(mk_element(
                1,
                110.0,
                70.0 + i as f32 * 12.0,
                280.0,
                3.0,
                10.0,
                "row",
                false,
                false,
            ));
        }
        let stats = run_pipeline(&elements, &geometry);
        assert_eq!(stats.pages[0].n_peaks_y, 30);
        assert!(
            stats.pages[0].y_peak_cv < 0.10,
            "y_peak_cv = {}",
            stats.pages[0].y_peak_cv
        );
    }

    // -- 15. Page-level mutually-exclusive composition ------------------------

    #[test]
    fn page_composition_mutex_sums() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let elements = vec![
            mk_element(
                1,
                110.0,
                100.0,
                80.0,
                12.0,
                10.0,
                "italic words",
                true,
                false,
            ),
            mk_element(1, 200.0, 100.0, 80.0, 12.0, 10.0, "bold words", false, true),
            mk_element(1, 110.0, 120.0, 80.0, 12.0, 10.0, "ital bold", true, true),
            mk_element(
                1,
                200.0,
                120.0,
                80.0,
                12.0,
                10.0,
                "normal words",
                false,
                false,
            ),
        ];
        let stats = run_pipeline(&elements, &geometry);
        let p = &stats.pages[0];
        assert_eq!(
            p.italic_tokens + p.bold_tokens + p.normal_tokens,
            p.n_tokens
        );
        assert_eq!(p.n_tokens, 8);
    }

    // -- 16. Region label matches RegionStats leaf label ----------------------

    #[test]
    fn region_label_matches_regionstats() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![300.0]);
        let mut elements = Vec::new();
        // Top section single-col y=[80, 300].
        let mut y = 80.0;
        while y + 14.0 <= 300.0 {
            elements.push(mk_element(
                1,
                110.0,
                y,
                380.0,
                14.0,
                10.0,
                "top section",
                false,
                false,
            ));
            y += 14.0;
        }
        // Bottom two-col y=[350, 700].
        y = 350.0;
        while y + 14.0 <= 700.0 {
            elements.push(mk_element(
                1, 110.0, y, 170.0, 14.0, 10.0, "left col", false, false,
            ));
            elements.push(mk_element(
                1,
                320.0,
                y,
                170.0,
                14.0,
                10.0,
                "right col",
                false,
                false,
            ));
            y += 14.0;
        }

        // Build the full RegionStats too, to compare labels.
        let mut region_b = RegionStatsBuilder::default();
        let mut page_b = PageStatsBuilder::default();
        for e in &elements {
            region_b.observe(e);
            page_b.observe(e);
        }
        let region = region_b.finalize(&FinalizationContext {
            font: None,
            geometry: Some(&geometry),
            region: None,
        });
        let stats = page_b.finalize(&FinalizationContext {
            font: None,
            geometry: Some(&geometry),
            region: Some(&region),
        });

        // Walk leaves of the RegionStats tree, compare labels in order.
        let mut leaf_labels: Vec<String> = Vec::new();
        fn walk(r: &Region, out: &mut Vec<String>) {
            if r.children.is_empty() {
                out.push(r.label.clone());
            } else {
                for c in &r.children {
                    walk(c, out);
                }
            }
        }
        for pr in &region.per_page {
            walk(&pr.root, &mut leaf_labels);
        }
        let sig_labels: Vec<String> = stats
            .regions
            .iter()
            .map(|r| r.region_label.clone())
            .collect();
        assert_eq!(leaf_labels, sig_labels);
    }

    // -- 17. Empty input → default --------------------------------------------

    #[test]
    fn empty_input_yields_default() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![]);
        let stats = run_pipeline(&[], &geometry);
        assert!(stats.pages.is_empty());
        assert!(stats.regions.is_empty());
    }

    // -- 18. Determinism: same input → byte-identical JSON --------------------

    #[test]
    fn determinism_byte_identical_json() {
        let geometry = mk_geometry(60.0, 720.0, 100.0, 500.0, vec![300.0]);
        let mut elements = Vec::new();
        let mut y = 80.0;
        while y + 14.0 <= 700.0 {
            elements.push(mk_element(
                1, 110.0, y, 170.0, 14.0, 10.0, "left", false, false,
            ));
            elements.push(mk_element(
                1, 320.0, y, 170.0, 14.0, 10.0, "right", false, false,
            ));
            y += 14.0;
        }
        let a = serde_json::to_string(&run_pipeline(&elements, &geometry)).unwrap();
        let b = serde_json::to_string(&run_pipeline(&elements, &geometry)).unwrap();
        assert_eq!(a, b);
    }

    // -- 19. Smoke: AnalysisBuilder full pipeline yields populated PageStats --

    #[test]
    fn analysis_builder_smoke() {
        use crate::analytics::builder::AnalysisBuilder;
        let mut elements = Vec::new();
        // 2 pages × 30 body lines so geometry has enough signal to detect
        // header_y / doc_footer_y / margins.
        for p in 1..=2 {
            for i in 0..30 {
                elements.push(mk_element(
                    p,
                    100.0,
                    80.0 + i as f32 * 14.0,
                    400.0,
                    14.0,
                    10.0,
                    "lorem ipsum dolor",
                    false,
                    false,
                ));
            }
        }
        let mut b = AnalysisBuilder::new();
        for e in &elements {
            b.observe(e);
        }
        let analysis = b.finalize();
        assert_eq!(analysis.page_stats.pages.len(), 2);
        // At least some regions populated (depends on geometry detection
        // succeeding, which it does on this synthetic shape).
        // No strict count — geometry edge cases on synthetic data are
        // possible; what matters is no panic and shape validity.
        for r in &analysis.page_stats.regions {
            assert!(!r.region_label.is_empty());
        }
    }
}
