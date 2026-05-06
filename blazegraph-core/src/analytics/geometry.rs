// Geometry-level descriptive statistics: header, doc-footer, margins,
// per-page footer, column layout, and a downsampled bbox-stacking heatmap.
// Ported from `scripts/heatmap_prototype.py` (canonical for algorithm
// behaviour). Block 06 of the document-analytics flow — see
// `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`
// (Block 03 spec) and `docs/P2/core/handoffs/2026-05-01-block03-geometry-header-line.md`
// for the type-shape contract and the empirical rationale behind every config
// default. Validation oracle: `scripts/output/{stem}/geometry.json`.

use serde::{Deserialize, Serialize};

use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::{BoundingBox, PdfTextElement};

// ---------------------------------------------------------------------------
// Output type shape
// ---------------------------------------------------------------------------

/// Document-level geometry: five lines defining six regions on the page,
/// plus the column layout and a downsampled persistent heatmap.
///
/// Coordinate convention: PDF/Tika points, y increasing downward.
/// `header_y` < `doc_footer_y`; `left_x` < `right_x`. Body region is
/// `header_y <= y < doc_footer_y` ∩ `left_x < x < right_x`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeometryStats {
    /// Top boundary of body. Body at y > header_y; header zone at y < header_y.
    pub header_y: f32,

    /// Bottom boundary of body at the document level. Body at y < doc_footer_y;
    /// below is the doc-level footer zone (running footer chrome + bottom margin).
    /// Per-page footers (variable footnotes) are captured separately via
    /// `per_page_footer_y` and are always at y <= doc_footer_y.
    pub doc_footer_y: f32,

    /// Horizontal body bounds. Body at left_x < x < right_x; outside is margin.
    pub left_x: f32,
    pub right_x: f32,

    /// Per-page footer line, indexed by sample-page order (one entry per page
    /// in `source_pages`). `Some(y)`: per-page body bottom; always
    /// `<= doc_footer_y` by construction. `None`: page has no detectable body
    /// in its bottom half (e.g., references-only page, near-empty page, cover
    /// page with no body content). Downstream consumers should fall back to
    /// `doc_footer_y` for `None` entries.
    pub per_page_footer_y: Vec<Option<f32>>,

    /// Number of pages stacked into the heatmap and analyzed for per-page
    /// footers. Capped at config.page_analysis_count; may be less for short docs.
    pub source_pages: u32,

    /// Page dimensions used for the heatmap (max across sampled pages).
    pub page_dimensions: PageDimensions,

    /// Detected column structure inside the body region.
    pub column_layout: ColumnLayout,

    /// Downsampled persistence of the bbox-stacking heatmap. See
    /// `DensityGrid` doc. Available to downstream pipes (e.g., Page
    /// Outlier Detection) as the document's spatial signature.
    pub heatmap: DensityGrid,

    /// Diagnostic summary of the walks and per-page analysis.
    pub diagnostic: GeometryDiagnostic,
}

/// Downsampled persistence of the per-page bbox-stacking heatmap. Cell value
/// is the SUM of full-resolution cell counts in the corresponding
/// `cell_size × cell_size` region. Sum (not mean, not max) is what the
/// consumer needs: weighted-overlap scores like
/// `sum_over_cells(page_covers_cell × cell_density)` compose directly. Mean
/// discards the cell-area normalization; max discards multi-page consistency.
///
/// `u16` fits: max value = `cell_size² × page_analysis_count` (at default
/// 8 × 8 × 10 = 640, well below 65535). Implementations clip to `u16::MAX`
/// defensively in case overlapping bboxes push a cell past the steady-state
/// upper bound.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DensityGrid {
    /// Page-coordinate pt per cell. From `config.heatmap_cell_size`.
    pub cell_size: u32,
    /// `ceil(page_dimensions.width / cell_size)`.
    pub cols: u32,
    /// `ceil(page_dimensions.height / cell_size)`.
    pub rows: u32,
    /// Row-major: `cells[row * cols + col]`.
    pub cells: Vec<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColumnLayout {
    /// Number of reading columns detected in the body region.
    /// 1 = single-column. 2 = typical two-column paper. >2 = rare.
    pub column_count: u32,

    /// X-positions (in pt) of inter-column dividers (gap centers). Length is
    /// always `column_count - 1`. Empty vec for single-column layouts.
    pub column_dividers: Vec<f32>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PageDimensions {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeometryDiagnostic {
    /// Maximum cell value in the full-resolution heatmap (= source_pages when
    /// text fully aligned across pages).
    pub heatmap_max: u32,
    /// Reason returned by each doc-level walk. Useful for post-hoc inspection.
    /// Examples: "found-significant-gap", "found-chrome-then-tail",
    /// "gap-to-page-bottom", "no-significant-gap-found".
    pub header_reason: String,
    pub doc_footer_reason: String,
    pub left_margin_reason: String,
    pub right_margin_reason: String,
    /// Column-detection metadata: peak, drop_threshold, high_threshold (the
    /// flanking-cell threshold). Useful for tuning.
    pub column_peak: f32,
    pub column_drop_threshold: f32,
    pub column_high_threshold: f32,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tunable parameters for geometry-level statistics.
///
/// Defaults are baked in for now; YAML wiring lands when consumers are
/// re-tuned. Every default is empirically validated on the 6-PDF corpus
/// (academic 2-col, single-col academic, RFC, EU legal with article
/// numeration, UK legal multi-line header, math-heavy paper).
#[derive(Debug, Clone)]
pub struct GeometryStatsConfig {
    /// Number of pages to load into the analysis window. Pages are taken
    /// as a linear span from the start of the document (pages [0, N)),
    /// not random sample. Default: 10. Validated on the 6-PDF corpus;
    /// "first 10" is the regime where the column-divider X-projection
    /// signal is cleanest (gpt2's full-document projection has full-width
    /// spanners that dilute the gutter past detection threshold).
    pub page_analysis_count: usize,

    /// Minimum sustained-gap length (in pt rows) for the header walk and
    /// the doc-footer walk's "sustained gap" branch. Default: 15.
    /// Tuned to skip body inter-line / inter-paragraph gaps but catch
    /// header→body and body→footer gaps.
    pub min_gap_rows: usize,

    /// Minimum sustained-gap length (in pt cols) for the margin walks.
    /// Default: 35. Wider than min_gap_rows because the body has more
    /// internal X-direction structure (inter-column gutters ~20-25pt;
    /// numeration→body gaps in legal layouts ~20-30pt).
    pub min_gap_cols: usize,

    /// Maximum content-patch height (in pt rows) the doc-footer walk
    /// classifies as "footer chrome" rather than "body continuation".
    /// Default: 50. Above this, a content patch is treated as body
    /// (the gap preceding it is body-internal, not body-bottom).
    pub max_footer_extent: usize,

    /// Per-page footer tolerance (in pt). A row is body-sized if its
    /// average per-token font_size >= page_median - tolerance.
    /// Default: 1.0. Wider tolerance admits embedded equations/captions
    /// as body; narrower excludes more, risking false-positive footer
    /// detection on body rows with mixed sizing.
    pub per_page_tolerance: f32,

    /// Per-page minimum-token-size filter. Tokens with font_size below
    /// this value are excluded from per-page analysis. Default: 1.0.
    /// Filters PDF rendering artifacts (e.g., size=0.1 parens around
    /// inline math glyphs in the Shannon paper) that contaminate per-row
    /// size averages without contributing visible content.
    pub min_token_size: f32,

    /// Column-layout drop threshold as fraction of body-row X-projection
    /// peak. A column position is "low" when sum_per_col[x] < ratio * peak.
    /// Default: 0.10. Tighter (e.g., 0.05) misses gpt2-style inter-col
    /// gutters that have ~5-7% page-1 content bleed; wider (e.g., 0.25)
    /// risks catching layout taper.
    pub column_drop_ratio: f32,

    /// Minimum sustained low-run length (in pt cols) for a column-divider
    /// candidate. Default: 8. Smaller than min_gap_cols because inter-col
    /// gutters are typically 10-30pt wide, narrower than outer margins.
    pub column_min_drop_cols: usize,

    /// Flanking-cell threshold as fraction of peak. A divider candidate
    /// must have its IMMEDIATE-NEIGHBOR cells (drop_start - 1 and
    /// drop_end + 1) at >= ratio * peak. Default: 0.50. Rejects edge
    /// taper (no body density on margin side) and indented-list markers
    /// (no body density on margin side). The drop must be SANDWICHED
    /// between body-density text on both sides — a real reading-flow
    /// break, not a boundary artifact.
    pub column_high_ratio: f32,

    /// Cell size (in pt) for the persisted `DensityGrid` downsample of
    /// the bbox-stacking heatmap. Default: 8. The full-resolution 1pt
    /// heatmap drives line detection (header/footer/margins/columns);
    /// this downsampled grid persists in the output for downstream
    /// consumers (Page Outlier Detection). 8pt cells (~7.6 KB per
    /// letter page) preserve enough resolution to distinguish body row
    /// from inter-line gap while collapsing PDF rendering noise.
    pub heatmap_cell_size: u32,
}

impl Default for GeometryStatsConfig {
    fn default() -> Self {
        Self {
            page_analysis_count: 10,
            min_gap_rows: 15,
            min_gap_cols: 35,
            max_footer_extent: 50,
            per_page_tolerance: 1.0,
            min_token_size: 1.0,
            column_drop_ratio: 0.10,
            column_min_drop_cols: 8,
            column_high_ratio: 0.50,
            heatmap_cell_size: 8,
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Per-page accumulation. Bboxes drive the heatmap (presence-based, no size
/// filter). Tokens drive the per-page footer walk (filtered by
/// rotation/text/min-size). The two views are kept separate to stay faithful
/// to the prototype's filter semantics.
#[derive(Debug, Default)]
struct PageAccumulator {
    page_number: u32,
    width: f32,
    height: f32,
    /// Bboxes for heatmap construction. Filter: rotation == 0 only.
    bboxes: Vec<BoundingBox>,
    /// Tokens for per-page footer analysis. Filter: rotation == 0,
    /// non-empty trimmed text, font_size >= config.min_token_size.
    tokens: Vec<TokenForGeometry>,
}

#[derive(Debug, Clone)]
struct TokenForGeometry {
    bbox: BoundingBox,
    font_size: f32,
}

/// Builder for geometry-level descriptive statistics.
///
/// Constructed once per document. Call [`observe`] for every
/// [`PdfTextElement`] in reading order, then [`finalize`] to produce a
/// [`GeometryStats`].
#[derive(Debug, Default)]
pub struct GeometryStatsBuilder {
    config: GeometryStatsConfig,
    /// Pages in observation order. The first `config.page_analysis_count`
    /// distinct page numbers are kept; further pages are dropped.
    pages: Vec<PageAccumulator>,
}

impl GeometryStatsBuilder {
    /// Construct a builder with explicit config. The `AnalysisBuilder` in
    /// `builder.rs` uses `GeometryStatsBuilder::default()` which picks up
    /// `GeometryStatsConfig::default()`.
    pub fn new(config: GeometryStatsConfig) -> Self {
        Self {
            config,
            pages: Vec::new(),
        }
    }

    /// Index of the page in `pages` for this element, or `None` if the
    /// page-window is full and this is a new page.
    fn page_slot(&mut self, element: &PdfTextElement) -> Option<usize> {
        let page_number = element.page_number();
        if let Some(idx) = self.pages.iter().position(|p| p.page_number == page_number) {
            return Some(idx);
        }
        if self.pages.len() >= self.config.page_analysis_count {
            return None;
        }
        let bbox = element.bounding_box();
        // The accumulator's width/height tracks the max bbox extent observed
        // on this page so far; final page dimensions come from these via
        // ceil() in build_heatmap (we don't have access to true page rect
        // in PdfTextElement — content extent is the available proxy and
        // matches what Tika reports).
        let _ = bbox;
        self.pages.push(PageAccumulator {
            page_number,
            width: 0.0,
            height: 0.0,
            bboxes: Vec::new(),
            tokens: Vec::new(),
        });
        Some(self.pages.len() - 1)
    }
}

impl Statistic for GeometryStatsBuilder {
    type Output = GeometryStats;
    const NAME: &'static str = "geometry";

    fn observe(&mut self, element: &PdfTextElement) {
        if element.rotation() != 0 {
            return;
        }
        let bbox = element.bounding_box().clone();
        let font_size = element.style_info.font_size;
        let text = &element.text;
        let page_w = element.placement.page_width;
        let page_h = element.placement.page_height;

        let Some(idx) = self.page_slot(element) else {
            return;
        };
        let page = &mut self.pages[idx];

        // Page dimensions: prefer Tika's `<div class="page-meta" data-width
        // data-height />` (sourced via Placement.page_{width,height}). Fall
        // back to bbox-content extent for c2 caches written before the
        // page-meta tag landed (page_width/page_height = 0.0) and for unit
        // tests that synthesize PdfTextElement directly. The fallback
        // under-approximates the true page rect — adequate for header /
        // margin / column walks but biases doc_footer_y upward toward the
        // deepest content row.
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

        // Heatmap input: every bbox (no size filter).
        page.bboxes.push(bbox.clone());

        // Per-page footer input: trimmed-non-empty text + size >= min_token_size.
        if font_size >= self.config.min_token_size && !text.trim().is_empty() {
            page.tokens.push(TokenForGeometry { bbox, font_size });
        }
    }

    fn finalize(self, ctx: &FinalizationContext<'_>) -> Self::Output {
        // Pull the document-level body-size signal from FontStats when it has
        // observations. The default FontStats value (12.0) on an empty
        // document is not meaningful — guard with a `font_size_counts`
        // non-empty check so synthetic tests (which don't run FontStats)
        // and empty docs both fall back to the per-page median.
        let doc_body_size = ctx.font.and_then(|f| {
            if f.font_size_counts.is_empty() {
                None
            } else {
                Some(f.most_common_font_size)
            }
        });
        finalize_geometry(self.pages, &self.config, doc_body_size)
    }
}

// ---------------------------------------------------------------------------
// Finalization orchestrator
// ---------------------------------------------------------------------------

fn finalize_geometry(
    pages: Vec<PageAccumulator>,
    config: &GeometryStatsConfig,
    doc_body_size: Option<f32>,
) -> GeometryStats {
    if pages.is_empty() {
        return GeometryStats::default();
    }

    let (heatmap, width, height) = build_heatmap(&pages);
    let n_pages = pages.len() as u32;

    let header = find_header_line(&heatmap, height, config.min_gap_rows);
    let footer = find_footer_line(
        &heatmap,
        height,
        config.min_gap_rows,
        config.max_footer_extent,
    );
    let body_y_start = header.line.min(footer.line);
    let body_y_end = header.line.max(footer.line);
    let left = find_left_margin(
        &heatmap,
        width,
        config.min_gap_cols,
        body_y_start,
        body_y_end,
    );
    let right = find_right_margin(
        &heatmap,
        width,
        config.min_gap_cols,
        body_y_start,
        body_y_end,
    );

    let column_layout_result = find_column_layout(
        &heatmap,
        header.line,
        footer.line,
        left.line,
        right.line,
        config.column_drop_ratio,
        config.column_min_drop_cols,
        config.column_high_ratio,
    );

    let per_page_footer_y = pages
        .iter()
        .map(|p| {
            find_per_page_footer_line(
                p,
                footer.line,
                config.per_page_tolerance,
                config.min_token_size,
                doc_body_size,
            )
        })
        .collect();

    let heatmap_max = heatmap
        .iter()
        .flat_map(|row| row.iter().copied())
        .max()
        .unwrap_or(0);

    let density_grid =
        downsample_to_density_grid(&heatmap, width, height, config.heatmap_cell_size);

    GeometryStats {
        header_y: header.line as f32,
        doc_footer_y: footer.line as f32,
        left_x: left.line as f32,
        right_x: right.line as f32,
        per_page_footer_y,
        source_pages: n_pages,
        page_dimensions: PageDimensions {
            width: width as f32,
            height: height as f32,
        },
        column_layout: ColumnLayout {
            column_count: column_layout_result.column_count,
            column_dividers: column_layout_result.column_dividers,
        },
        heatmap: density_grid,
        diagnostic: GeometryDiagnostic {
            heatmap_max,
            header_reason: header.reason,
            doc_footer_reason: footer.reason,
            left_margin_reason: left.reason,
            right_margin_reason: right.reason,
            column_peak: column_layout_result.peak,
            column_drop_threshold: column_layout_result.drop_threshold,
            column_high_threshold: column_layout_result.high_threshold,
        },
    }
}

// ---------------------------------------------------------------------------
// Heatmap construction
// ---------------------------------------------------------------------------

/// Build a 1pt-resolution bbox-stacking heatmap. Cell `(y, x)` counts the
/// number of pages on which at least one non-rotated bbox covers `(y, x)`.
/// Ported from `heatmap_prototype.py::build_heatmap`.
fn build_heatmap(pages: &[PageAccumulator]) -> (Vec<Vec<u32>>, usize, usize) {
    let max_w = pages
        .iter()
        .map(|p| p.width.ceil() as usize)
        .max()
        .unwrap_or(0);
    let max_h = pages
        .iter()
        .map(|p| p.height.ceil() as usize)
        .max()
        .unwrap_or(0);

    if max_w == 0 || max_h == 0 {
        return (vec![vec![0; max_w.max(1)]; max_h.max(1)], max_w, max_h);
    }

    let mut heatmap = vec![vec![0u32; max_w]; max_h];

    for page in pages {
        // Per-page presence mask — flat row-major bool grid.
        let mut mask = vec![false; max_w * max_h];
        for bbox in &page.bboxes {
            let xa = clamp_usize(bbox.x.floor() as i64, 0, max_w as i64);
            let xb = clamp_usize((bbox.x + bbox.width).ceil() as i64, 0, max_w as i64);
            let ya = clamp_usize(bbox.y.floor() as i64, 0, max_h as i64);
            let yb = clamp_usize((bbox.y + bbox.height).ceil() as i64, 0, max_h as i64);
            if xb > xa && yb > ya {
                for y in ya..yb {
                    let row = y * max_w;
                    mask[row + xa..row + xb].fill(true);
                }
            }
        }
        for (y, row) in heatmap.iter_mut().enumerate().take(max_h) {
            let base = y * max_w;
            for (x, cell) in row.iter_mut().enumerate().take(max_w) {
                if mask[base + x] {
                    *cell += 1;
                }
            }
        }
    }

    (heatmap, max_w, max_h)
}

fn clamp_usize(v: i64, lo: i64, hi: i64) -> usize {
    v.max(lo).min(hi) as usize
}

// ---------------------------------------------------------------------------
// Header / footer / margin walks
// ---------------------------------------------------------------------------

struct WalkResult {
    line: usize,
    reason: String,
}

/// Walk UP from the middle and stop at the first sustained low-run of
/// `min_gap_rows` rows. Return the BOTTOM of that gap (the row closest to
/// the middle within the gap). Ported from `find_header_line` in
/// `heatmap_prototype.py`.
fn find_header_line(heatmap: &[Vec<u32>], height: usize, min_gap_rows: usize) -> WalkResult {
    if height == 0 {
        return WalkResult {
            line: 0,
            reason: "empty-heatmap".to_string(),
        };
    }
    let middle = height / 2;
    let sum_per_row = sum_rows(heatmap);

    let mut gap_bottom: Option<usize> = None;
    let mut gap_length: usize = 0;
    let mut y = middle;
    loop {
        if sum_per_row[y] > 0 {
            gap_bottom = None;
            gap_length = 0;
        } else {
            if gap_bottom.is_none() {
                gap_bottom = Some(y);
            }
            gap_length += 1;
            if gap_length >= min_gap_rows {
                return WalkResult {
                    line: gap_bottom.unwrap(),
                    reason: "found-significant-gap".to_string(),
                };
            }
        }
        if y == 0 {
            break;
        }
        y -= 1;
    }
    WalkResult {
        line: 0,
        reason: "no-significant-gap-found".to_string(),
    }
}

/// Walk DOWN from the middle, distinguishing body→chrome→tail (footer chrome
/// present) from body→sustained-gap (no chrome) from body-internal gap+patch
/// (skip and keep walking). Returns the TOP of the body-bottom gap.
/// Ported from `find_footer_line` in `heatmap_prototype.py`.
fn find_footer_line(
    heatmap: &[Vec<u32>],
    height: usize,
    min_gap_rows: usize,
    max_footer_extent: usize,
) -> WalkResult {
    if height == 0 {
        return WalkResult {
            line: 0,
            reason: "empty-heatmap".to_string(),
        };
    }
    let middle = height / 2;
    let sum_per_row = sum_rows(heatmap);

    let mut y = middle;
    while y < height {
        // Skip body content
        while y < height && sum_per_row[y] > 0 {
            y += 1;
        }
        if y >= height {
            return WalkResult {
                line: height.saturating_sub(1),
                reason: "no-gap-found".to_string(),
            };
        }

        // In a gap — record top, measure length
        let gap_top = y;
        while y < height && sum_per_row[y] == 0 {
            y += 1;
        }
        let gap_length = y - gap_top;

        if y >= height {
            // Gap extends to page bottom — body ended at gap_top, no chrome.
            return WalkResult {
                line: gap_top,
                reason: "gap-to-page-bottom".to_string(),
            };
        }

        // Peek ahead: how big is the content patch beyond the gap?
        let content_top = y;
        while y < height && sum_per_row[y] > 0 {
            y += 1;
        }
        let content_extent = y - content_top;

        if content_extent <= max_footer_extent {
            // Chrome-sized patch. Verify a sustained tail follows (not body again).
            let tail_start = y;
            while y < height && sum_per_row[y] == 0 {
                y += 1;
            }
            let tail_length = y - tail_start;
            if y >= height || tail_length >= min_gap_rows {
                return WalkResult {
                    line: gap_top,
                    reason: "found-chrome-then-tail".to_string(),
                };
            }
            // Tail too short — chrome-sized patch was actually a brief body
            // interruption. Keep walking.
        } else if gap_length >= min_gap_rows {
            // Big content patch BUT preceded by a sustained gap. The sustained
            // gap is the body-bottom signal regardless of what comes after.
            return WalkResult {
                line: gap_top,
                reason: "found-significant-gap".to_string(),
            };
        }
        // Else: small gap + large content = body-internal gap. y is already
        // past the body region; loop continues.
    }

    WalkResult {
        line: height.saturating_sub(1),
        reason: "no-gap-found".to_string(),
    }
}

/// Walk LEFT from the horizontal middle, return the RIGHT edge of the first
/// sustained low-run of `min_gap_cols` columns. The X-projection is computed
/// over body rows only (`body_y_start..body_y_end`), so wide running headers
/// or footers don't bias the margin outward. Ported from `find_left_margin`.
fn find_left_margin(
    heatmap: &[Vec<u32>],
    width: usize,
    min_gap_cols: usize,
    body_y_start: usize,
    body_y_end: usize,
) -> WalkResult {
    if width == 0 {
        return WalkResult {
            line: 0,
            reason: "empty-heatmap".to_string(),
        };
    }
    let middle = width / 2;
    let sum_per_col = sum_cols_in_y_range(heatmap, body_y_start, body_y_end, width);

    let mut gap_right_edge: Option<usize> = None;
    let mut gap_length: usize = 0;
    let mut x = middle;
    loop {
        if sum_per_col[x] > 0 {
            gap_right_edge = None;
            gap_length = 0;
        } else {
            if gap_right_edge.is_none() {
                gap_right_edge = Some(x);
            }
            gap_length += 1;
            if gap_length >= min_gap_cols {
                return WalkResult {
                    line: gap_right_edge.unwrap(),
                    reason: "found-significant-gap".to_string(),
                };
            }
        }
        if x == 0 {
            break;
        }
        x -= 1;
    }
    WalkResult {
        line: 0,
        reason: "no-significant-gap-found".to_string(),
    }
}

/// Mirror of `find_left_margin`. Walk RIGHT, return LEFT edge of first
/// sustained gap. Ported from `find_right_margin`.
fn find_right_margin(
    heatmap: &[Vec<u32>],
    width: usize,
    min_gap_cols: usize,
    body_y_start: usize,
    body_y_end: usize,
) -> WalkResult {
    if width == 0 {
        return WalkResult {
            line: 0,
            reason: "empty-heatmap".to_string(),
        };
    }
    let middle = width / 2;
    let sum_per_col = sum_cols_in_y_range(heatmap, body_y_start, body_y_end, width);

    let mut gap_left_edge: Option<usize> = None;
    let mut gap_length: usize = 0;
    let mut x = middle;
    while x < width {
        if sum_per_col[x] > 0 {
            gap_left_edge = None;
            gap_length = 0;
        } else {
            if gap_left_edge.is_none() {
                gap_left_edge = Some(x);
            }
            gap_length += 1;
            if gap_length >= min_gap_cols {
                return WalkResult {
                    line: gap_left_edge.unwrap(),
                    reason: "found-significant-gap".to_string(),
                };
            }
        }
        x += 1;
    }
    WalkResult {
        line: width.saturating_sub(1),
        reason: "no-significant-gap-found".to_string(),
    }
}

fn sum_rows(heatmap: &[Vec<u32>]) -> Vec<u64> {
    heatmap
        .iter()
        .map(|row| row.iter().map(|&v| v as u64).sum())
        .collect()
}

fn sum_cols_in_y_range(
    heatmap: &[Vec<u32>],
    y_start: usize,
    y_end: usize,
    width: usize,
) -> Vec<u64> {
    let mut sums = vec![0u64; width];
    let height = heatmap.len();
    let lo = y_start.min(height);
    let hi = y_end.min(height);
    for row in &heatmap[lo..hi] {
        for (x, &v) in row.iter().enumerate().take(width) {
            sums[x] += v as u64;
        }
    }
    sums
}

// ---------------------------------------------------------------------------
// Column layout
// ---------------------------------------------------------------------------

struct ColumnLayoutResult {
    column_count: u32,
    column_dividers: Vec<f32>,
    peak: f32,
    drop_threshold: f32,
    high_threshold: f32,
}

/// Detect reading columns via sharp-drop analysis in the body-row X-projection.
/// A divider is a sustained low-run flanked on BOTH sides by body-density
/// columns. Ported from `find_column_layout`.
#[allow(clippy::too_many_arguments)]
fn find_column_layout(
    heatmap: &[Vec<u32>],
    header_y: usize,
    doc_footer_y: usize,
    left_x: usize,
    right_x: usize,
    drop_ratio: f32,
    min_drop_cols: usize,
    high_ratio: f32,
) -> ColumnLayoutResult {
    if right_x <= left_x || heatmap.is_empty() || heatmap[0].is_empty() {
        return ColumnLayoutResult {
            column_count: 1,
            column_dividers: Vec::new(),
            peak: 0.0,
            drop_threshold: 0.0,
            high_threshold: 0.0,
        };
    }

    let width = heatmap[0].len();
    let sum_per_col = sum_cols_in_y_range(heatmap, header_y, doc_footer_y, width);

    let body_lo = left_x.min(width);
    let body_hi = (right_x + 1).min(width);
    if body_hi <= body_lo {
        return ColumnLayoutResult {
            column_count: 1,
            column_dividers: Vec::new(),
            peak: 0.0,
            drop_threshold: 0.0,
            high_threshold: 0.0,
        };
    }
    let peak = sum_per_col[body_lo..body_hi]
        .iter()
        .copied()
        .max()
        .unwrap_or(0) as f32;
    let drop_threshold = drop_ratio * peak;
    let high_threshold = high_ratio * peak;

    let mut dividers: Vec<f32> = Vec::new();
    let mut in_drop = false;
    let mut drop_start: Option<usize> = None;

    let close_drop = |start: usize, end_exclusive: usize, dividers: &mut Vec<f32>| {
        let drop_end = end_exclusive.saturating_sub(1);
        let drop_length = end_exclusive.saturating_sub(start);
        if drop_length < min_drop_cols {
            return;
        }
        let left_ok = start > left_x && (sum_per_col[start - 1] as f32) >= high_threshold;
        let right_ok = drop_end < right_x && (sum_per_col[drop_end + 1] as f32) >= high_threshold;
        if left_ok && right_ok {
            dividers.push((start as f32 + drop_end as f32) / 2.0);
        }
    };

    let scan_hi = right_x.min(width.saturating_sub(1));
    for (x, &v) in sum_per_col
        .iter()
        .enumerate()
        .take(scan_hi + 1)
        .skip(left_x)
    {
        if (v as f32) < drop_threshold {
            if !in_drop {
                drop_start = Some(x);
                in_drop = true;
            }
        } else if in_drop {
            if let Some(start) = drop_start {
                close_drop(start, x, &mut dividers);
            }
            in_drop = false;
            drop_start = None;
        }
    }
    if in_drop {
        if let Some(start) = drop_start {
            close_drop(start, right_x + 1, &mut dividers);
        }
    }

    ColumnLayoutResult {
        column_count: (dividers.len() as u32) + 1,
        column_dividers: dividers,
        peak,
        drop_threshold,
        high_threshold,
    }
}

// ---------------------------------------------------------------------------
// Per-page footer line (font-size-based)
// ---------------------------------------------------------------------------

/// Per-page footer detection by font-size transition. Walking UP from
/// `doc_footer_y`, find the first row where the average token font_size
/// meets the body-size threshold (within `tolerance`). That row is body;
/// the per-page footer line is one row below it. Ported from
/// `find_per_page_footer_line`.
///
/// `doc_body_size` is the document-level body size from FontStats — used as
/// the body reference when available. Falls back to the per-page median
/// when `None` (e.g. synthetic tests, or when FontStats is disabled). The
/// document-level reference is more robust on documents where the page's
/// median is dragged down by abundant non-body content (academic papers
/// with long footnote blocks, where Tika's per-segment span granularity
/// over-counts 8/9pt fragments and pulls the median below body size).
fn find_per_page_footer_line(
    page: &PageAccumulator,
    doc_footer_y: usize,
    tolerance: f32,
    min_token_size: f32,
    doc_body_size: Option<f32>,
) -> Option<f32> {
    let elements: Vec<&TokenForGeometry> = page
        .tokens
        .iter()
        .filter(|t| t.font_size >= min_token_size)
        .collect();
    if elements.is_empty() {
        return None;
    }

    let height = page.height.ceil() as usize;
    if height == 0 {
        return None;
    }
    let middle = height / 2;

    // Body-size reference: prefer the document-level signal from FontStats;
    // fall back to the per-page median when unavailable.
    let body_size = doc_body_size.unwrap_or_else(|| {
        let mut sizes: Vec<f32> = elements.iter().map(|t| t.font_size).collect();
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if sizes.len() % 2 == 1 {
            sizes[sizes.len() / 2]
        } else {
            let mid = sizes.len() / 2;
            (sizes[mid - 1] + sizes[mid]) / 2.0
        }
    });
    let threshold = body_size - tolerance;
    if threshold <= 0.0 {
        // Degenerate: no usable size gradient. Hits when Tika reports
        // uniform nominal `font_size=1.0` (common for CELEX-style legal
        // PDFs where the embedded font metrics don't expose real point
        // sizes). The per-page footer algorithm depends on a body→footer
        // size transition; without one, every content row passes the
        // threshold and the result is meaningless. Returning `None`
        // matches the established "no detectable body" contract —
        // consumers fall back to `doc_footer_y`.
        return None;
    }

    // Per-row token-size aggregates: each token contributes once per row
    // its bbox covers (floor..ceil).
    let mut row_sums = vec![0f64; height];
    let mut row_counts = vec![0u32; height];
    for t in &elements {
        let ya = t.bbox.y.floor().max(0.0) as usize;
        let yb = ((t.bbox.y + t.bbox.height).ceil() as usize).min(height);
        if yb > ya {
            for y in ya..yb {
                row_sums[y] += t.font_size as f64;
                row_counts[y] += 1;
            }
        }
    }

    // Walk UP from doc_footer_y, skip empty rows, stop at first body row.
    //
    // Strict `>` (vs the Python prototype's `>=`): when Tika's per-segment
    // span granularity drives the body-size reference downward by one pt
    // (e.g. attention's most_common_font_size=9 because Tika overcounts 9pt
    // body fragments vs PyMuPDF's per-glyph clusters), a tied threshold
    // would admit footer rows that share that one-pt margin. Strict `>`
    // requires body rows to be measurably above the body-size − tolerance
    // band; Tika 9pt body still passes (9 > 8) but Tika 8pt footers do not
    // (8 > 8 is false). Body rows that genuinely sit at exactly threshold
    // are extremely rare in real corpus data.
    let mut y = doc_footer_y.min(height.saturating_sub(1));
    while y >= middle {
        if row_counts[y] > 0 {
            let avg = (row_sums[y] / row_counts[y] as f64) as f32;
            if avg > threshold {
                return Some((y + 1) as f32);
            }
        }
        if y == 0 {
            break;
        }
        y -= 1;
    }
    None
}

// ---------------------------------------------------------------------------
// DensityGrid downsample
// ---------------------------------------------------------------------------

/// Downsample the full-resolution heatmap into a `DensityGrid` with cell
/// size `cell_size`. Each output cell holds the SUM of input cells in the
/// corresponding `cell_size × cell_size` region (clipped to `u16::MAX`
/// defensively). Sum semantics is what consumers need — see DensityGrid doc.
fn downsample_to_density_grid(
    heatmap: &[Vec<u32>],
    width: usize,
    height: usize,
    cell_size: u32,
) -> DensityGrid {
    if cell_size == 0 || width == 0 || height == 0 {
        return DensityGrid {
            cell_size: cell_size.max(1),
            cols: 0,
            rows: 0,
            cells: Vec::new(),
        };
    }
    let cs = cell_size as usize;
    let cols = width.div_ceil(cs);
    let rows = height.div_ceil(cs);
    let mut cells = vec![0u16; rows * cols];

    for out_row in 0..rows {
        let y_start = out_row * cs;
        let y_end = (y_start + cs).min(height);
        for out_col in 0..cols {
            let x_start = out_col * cs;
            let x_end = (x_start + cs).min(width);
            let mut sum: u32 = 0;
            for row in &heatmap[y_start..y_end] {
                for &v in &row[x_start..x_end] {
                    sum = sum.saturating_add(v);
                }
            }
            cells[out_row * cols + out_col] = sum.min(u16::MAX as u32) as u16;
        }
    }

    DensityGrid {
        cell_size,
        cols: cols as u32,
        rows: rows as u32,
        cells,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FontClass, Placement};

    /// Construct a synthetic `PdfTextElement` with explicit bbox + page +
    /// font_size + rotation. Style fields default to body-like values.
    #[allow(clippy::too_many_arguments)]
    fn make_element(
        page: u32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        font_size: f32,
        rotation: i32,
    ) -> PdfTextElement {
        PdfTextElement {
            text: "lorem".to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: "Times".to_string(),
                font_size,
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
                rotation,
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

    fn build_stats_with_config(
        elements: &[PdfTextElement],
        config: GeometryStatsConfig,
    ) -> GeometryStats {
        let mut b = GeometryStatsBuilder::new(config);
        for e in elements {
            b.observe(e);
        }
        b.finalize(&FinalizationContext::default())
    }

    fn build_stats(elements: &[PdfTextElement]) -> GeometryStats {
        build_stats_with_config(elements, GeometryStatsConfig::default())
    }

    /// Helper: create N pages of body content as a solid block (no inter-line
    /// gaps). bbox height equals row step so each row in [body_y_lo, body_y_hi)
    /// is filled — keeps the chrome state machine's "tail" check from
    /// triggering on synthetic body-internal patterns that wouldn't occur in
    /// real corpus data (where consecutive body lines pack tighter than
    /// `max_footer_extent`).
    fn synth_body_pages(
        n_pages: u32,
        page_w: f32,
        page_h: f32,
        body_x_ranges: &[(f32, f32)],
        body_y_lo: f32,
        body_y_hi: f32,
        line_h: f32,
    ) -> Vec<PdfTextElement> {
        let mut elements = Vec::new();
        for p in 1..=n_pages {
            let mut y = body_y_lo;
            while y + line_h <= body_y_hi {
                for (x0, x1) in body_x_ranges {
                    elements.push(make_element(p, *x0, y, x1 - x0, line_h, 10.0, 0));
                }
                y += line_h; // solid: no inter-line gap
            }
            // Anchor page extent
            elements.push(make_element(
                p,
                page_w - 0.1,
                page_h - 0.1,
                0.05,
                0.05,
                10.0,
                0,
            ));
        }
        elements
    }

    /// Inline body builder used by tests that need explicit y-control. Solid
    /// block of body bboxes filling EXACTLY [body_y_lo, body_y_hi). The final
    /// bbox is extended (or shrunk) so the band closes at `body_y_hi` — keeps
    /// test arithmetic simple (no off-by-line_h leftover gap before the
    /// declared body bottom).
    fn push_solid_body(
        elements: &mut Vec<PdfTextElement>,
        page: u32,
        body_y_lo: f32,
        body_y_hi: f32,
    ) {
        let line_h = 14.0;
        let mut y = body_y_lo;
        while y + line_h <= body_y_hi {
            elements.push(make_element(page, 100.0, y, 400.0, line_h, 10.0, 0));
            y += line_h;
        }
        // Cap to body_y_hi exactly with one final bbox covering [y, body_y_hi).
        if y < body_y_hi {
            elements.push(make_element(page, 100.0, y, 400.0, body_y_hi - y, 10.0, 0));
        }
    }

    // --- 1. Synthetic running header ------------------------------------------
    #[test]
    fn header_running_is_detected() {
        let mut elements = Vec::new();
        for p in 1..=10 {
            // running header at y=[35, 47]
            elements.push(make_element(p, 100.0, 35.0, 200.0, 12.0, 10.0, 0));
            // body solid block y=[80, 700]
            push_solid_body(&mut elements, p, 80.0, 700.0);
            // anchor
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        // gap_bottom = first low row going up from middle. With body starting
        // at y=80 the algorithm returns y=79 (the row just above body). The
        // semantic invariant: header_y < body_top, gap >= min_gap_rows.
        assert!(
            s.header_y >= 60.0 && s.header_y < 80.0,
            "header_y {} not in [60, 80) — should sit just above body_top=80",
            s.header_y
        );
        assert!(
            s.doc_footer_y >= 700.0,
            "doc_footer_y {} not at/above body bottom",
            s.doc_footer_y
        );
        assert_eq!(s.diagnostic.header_reason, "found-significant-gap");
    }

    // --- 2. No-header case ----------------------------------------------------
    #[test]
    fn header_top_margin_is_caught_when_no_header() {
        let elements = synth_body_pages(10, 600.0, 800.0, &[(100.0, 500.0)], 70.0, 700.0, 10.0);
        let s = build_stats(&elements);
        assert!(
            s.header_y >= 55.0 && s.header_y <= 70.0,
            "header_y {} not in [55, 70] for top-margin gap",
            s.header_y
        );
    }

    // --- 3. Multi-line header -------------------------------------------------
    #[test]
    fn header_multi_line_lands_below_lowest_band() {
        let mut elements = Vec::new();
        for p in 1..=10 {
            elements.push(make_element(p, 100.0, 30.0, 200.0, 15.0, 10.0, 0));
            elements.push(make_element(p, 100.0, 65.0, 200.0, 30.0, 10.0, 0));
            push_solid_body(&mut elements, p, 130.0, 700.0);
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        // Lowest header band ends at y=95 (65+30). Body starts at y=130. Gap
        // is [95, 130) = 35 rows. Walking up from middle, first low row hit
        // is y=129. Algorithm returns gap_bottom=129. Allow a small slack.
        assert!(
            s.header_y >= 95.0 && s.header_y < 130.0,
            "header_y {} not in [95, 130) — should sit just above body_top=130",
            s.header_y
        );
    }

    // --- 4. Footer chrome present ---------------------------------------------
    #[test]
    fn footer_chrome_lands_above_chrome() {
        let mut elements = Vec::new();
        for p in 1..=10 {
            // solid body to y=720
            push_solid_body(&mut elements, p, 80.0, 720.0);
            // page-number patch at y=[734, 747]
            elements.push(make_element(p, 280.0, 734.0, 40.0, 13.0, 10.0, 0));
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        // Body→gap→chrome→tail pattern. Body ends at y=716 (last bbox y=702
        // covers [702, 716)). Gap [716, 734) = 18 rows; chrome [734, 747);
        // tail [747, 791) = 44 rows. Algorithm returns gap_top=716.
        assert!(
            s.doc_footer_y >= 715.0 && s.doc_footer_y <= 735.0,
            "doc_footer_y {} not in [715, 735] (above chrome)",
            s.doc_footer_y
        );
        assert_eq!(s.diagnostic.doc_footer_reason, "found-chrome-then-tail");
    }

    // --- 5. No footer chrome --------------------------------------------------
    #[test]
    fn footer_no_chrome_returns_gap_to_bottom_or_significant_gap() {
        // Note: even with no "footer chrome" by the test's intent, the page
        // anchor element acts as a tiny near-bottom content patch and
        // legitimately matches the chrome-then-tail predicate. The line
        // value (gap_top) is what matters; the reason label is diagnostic.
        let mut elements = Vec::new();
        for p in 1..=10 {
            push_solid_body(&mut elements, p, 80.0, 750.0);
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        assert!(
            s.doc_footer_y >= 749.0 && s.doc_footer_y <= 760.0,
            "doc_footer_y {} not near body bottom (750)",
            s.doc_footer_y
        );
        let r = &s.diagnostic.doc_footer_reason;
        assert!(
            r == "gap-to-page-bottom"
                || r == "found-significant-gap"
                || r == "found-chrome-then-tail"
                || r == "no-gap-found",
            "unexpected reason: {r}"
        );
    }

    // --- 6. Two-column with margins -------------------------------------------
    #[test]
    fn margins_two_column_with_inter_gap_skipped() {
        let elements = synth_body_pages(
            10,
            612.0,
            792.0,
            &[(100.0, 290.0), (310.0, 500.0)],
            80.0,
            720.0,
            10.0,
        );
        let s = build_stats(&elements);
        assert!(
            s.left_x >= 85.0 && s.left_x <= 110.0,
            "left_x {} not in [85, 110]",
            s.left_x
        );
        assert!(
            s.right_x >= 490.0 && s.right_x <= 515.0,
            "right_x {} not in [490, 515]",
            s.right_x
        );
    }

    // --- 7. Body-row filter for margins ---------------------------------------
    #[test]
    fn margins_ignore_running_header_width() {
        let mut elements = Vec::new();
        for p in 1..=10 {
            // wide running header spanning x=[20, 580]
            elements.push(make_element(p, 20.0, 35.0, 560.0, 12.0, 10.0, 0));
            // narrower body x=[100, 500]
            let mut y = 100.0;
            while y < 700.0 {
                elements.push(make_element(p, 100.0, y, 400.0, 10.0, 10.0, 0));
                y += 14.0;
            }
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        assert!(
            s.left_x >= 90.0 && s.left_x <= 110.0,
            "left_x {} should track body, not header — running header polluted X-projection",
            s.left_x
        );
        assert!(
            s.right_x >= 490.0 && s.right_x <= 515.0,
            "right_x {} should track body, not header",
            s.right_x
        );
    }

    // --- 8. Rotation filter ---------------------------------------------------
    #[test]
    fn header_ignores_rotated_decorations_at_top() {
        let mut elements = Vec::new();
        for p in 1..=10 {
            // rotated decorative elements at y=[10, 20]
            elements.push(make_element(p, 50.0, 10.0, 20.0, 10.0, 10.0, 90));
            // body starting y=80
            let mut y = 80.0;
            while y < 700.0 {
                elements.push(make_element(p, 100.0, y, 400.0, 10.0, 10.0, 0));
                y += 14.0;
            }
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        assert!(
            s.header_y >= 55.0 && s.header_y <= 80.0,
            "header_y {} should be in top-margin range — rotated decoration polluted",
            s.header_y
        );
    }

    // --- 9. Short document ----------------------------------------------------
    #[test]
    fn source_pages_reflects_short_document() {
        let elements = synth_body_pages(3, 612.0, 792.0, &[(100.0, 500.0)], 80.0, 720.0, 10.0);
        let s = build_stats(&elements);
        assert_eq!(s.source_pages, 3);
    }

    // --- 10. Determinism ------------------------------------------------------
    #[test]
    fn determinism_same_input_byte_identical_json() {
        let elements = synth_body_pages(5, 612.0, 792.0, &[(100.0, 500.0)], 80.0, 720.0, 10.0);
        let a = serde_json::to_string(&build_stats(&elements)).unwrap();
        let b = serde_json::to_string(&build_stats(&elements)).unwrap();
        assert_eq!(a, b);
    }

    // --- 11. Per-page footer: smaller-font footnote block ---------------------
    #[test]
    fn per_page_footer_with_footnote_block() {
        let mut elements = Vec::new();
        for p in 1..=3 {
            let mut y = 80.0;
            while y < 600.0 {
                elements.push(make_element(p, 100.0, y, 400.0, 10.0, 10.0, 0));
                y += 14.0;
            }
            // 8pt footnote block at y=[620, 660]
            let mut yf = 620.0;
            while yf < 660.0 {
                elements.push(make_element(p, 100.0, yf, 400.0, 8.0, 8.0, 0));
                yf += 10.0;
            }
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        for (idx, ppf) in s.per_page_footer_y.iter().enumerate() {
            let v = ppf.unwrap_or(-1.0);
            assert!(
                (600.0..=625.0).contains(&v),
                "page {} per_page_footer_y={} not in [600, 625]",
                idx,
                v
            );
        }
    }

    // --- 12. Per-page: page with no footer ------------------------------------
    #[test]
    fn per_page_footer_no_footer_returns_doc_line() {
        let mut elements = Vec::new();
        for p in 1..=3 {
            let mut y = 80.0;
            while y < 740.0 {
                elements.push(make_element(p, 100.0, y, 400.0, 10.0, 10.0, 0));
                y += 14.0;
            }
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        for ppf in &s.per_page_footer_y {
            let v = ppf.unwrap_or(-1.0);
            assert!(
                (v - s.doc_footer_y).abs() < 5.0 || v >= s.doc_footer_y,
                "per_page_footer_y={} should be ~doc_footer_y={}",
                v,
                s.doc_footer_y
            );
        }
    }

    // --- 13. Per-page: embedded equation walked past --------------------------
    #[test]
    fn per_page_footer_skips_embedded_equation() {
        let mut elements = Vec::new();
        for p in 1..=3 {
            // body to y=550
            let mut y = 80.0;
            while y < 550.0 {
                elements.push(make_element(p, 100.0, y, 400.0, 10.0, 10.0, 0));
                y += 14.0;
            }
            // 8pt equation at y=[560, 590]
            let mut ye = 560.0;
            while ye < 590.0 {
                elements.push(make_element(p, 200.0, ye, 100.0, 8.0, 8.0, 0));
                ye += 10.0;
            }
            // body again to y=680
            let mut y2 = 600.0;
            while y2 < 680.0 {
                elements.push(make_element(p, 100.0, y2, 400.0, 10.0, 10.0, 0));
                y2 += 14.0;
            }
            // 8pt footer at y=[700, 740]
            let mut yf = 700.0;
            while yf < 740.0 {
                elements.push(make_element(p, 100.0, yf, 400.0, 8.0, 8.0, 0));
                yf += 10.0;
            }
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        for (idx, ppf) in s.per_page_footer_y.iter().enumerate() {
            let v = ppf.unwrap_or(-1.0);
            assert!(
                (680.0..=705.0).contains(&v),
                "page {} per_page_footer_y={} should land above the real footnote (680..705)",
                idx,
                v
            );
        }
    }

    // --- 14. Per-page: garbage-token filter -----------------------------------
    #[test]
    fn per_page_footer_filters_size_artifacts() {
        let mut elements = Vec::new();
        for p in 1..=3 {
            let mut y = 80.0;
            while y < 700.0 {
                elements.push(make_element(p, 100.0, y, 400.0, 10.0, 10.0, 0));
                // size=0.1 artifact tokens at the same row
                elements.push(make_element(p, 200.0, y + 2.0, 5.0, 1.0, 0.1, 0));
                y += 14.0;
            }
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        for (idx, ppf) in s.per_page_footer_y.iter().enumerate() {
            if let Some(v) = ppf {
                assert!(
                    *v >= 698.0 && *v <= 715.0,
                    "page {} per_page_footer_y={} should land near body bottom — artifacts polluted",
                    idx,
                    v
                );
            }
        }
    }

    // --- 15. Per-page: all-small page treats small text AS body (per-page median)
    #[test]
    fn per_page_footer_all_small_page_uses_per_page_median() {
        // The per-page algorithm uses the page's OWN median font size as the
        // body reference (not the document body size). An all-8pt page has
        // median=8pt; the algorithm classifies that 8pt content as "body" for
        // that page and returns Some(line). This test pins that contract —
        // future swap to document-level body size (when FontStats is plumbed
        // into the FinalizationContext) will need to revisit.
        let mut elements = Vec::new();
        // Page 1 (anchor): solid 10pt body for the doc-level walks.
        push_solid_body(&mut elements, 1, 80.0, 720.0);
        elements.push(make_element(1, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        // Page 2: only 8pt content in the bottom half.
        let mut yf = 500.0;
        while yf < 700.0 {
            elements.push(make_element(2, 100.0, yf, 400.0, 8.0, 8.0, 0));
            yf += 10.0;
        }
        elements.push(make_element(2, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        let s = build_stats(&elements);
        assert!(
            s.per_page_footer_y[1].is_some(),
            "per-page median treats 8pt as body for an all-8pt page"
        );
    }

    // --- 16. Per-page: cover page with no body in bottom half -----------------
    #[test]
    fn per_page_footer_cover_page_returns_none() {
        let mut elements = Vec::new();
        // Page 1: full body anchor.
        let mut y = 80.0;
        while y < 720.0 {
            elements.push(make_element(1, 100.0, y, 400.0, 10.0, 10.0, 0));
            y += 14.0;
        }
        elements.push(make_element(1, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        // Page 2 cover: title only at top; nothing below height/2.
        elements.push(make_element(2, 200.0, 100.0, 200.0, 30.0, 24.0, 0));
        elements.push(make_element(2, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        let s = build_stats(&elements);
        assert!(
            s.per_page_footer_y[1].is_none(),
            "cover page with no body in bottom half should yield None"
        );
    }

    // --- 17. Single-column doc ------------------------------------------------
    #[test]
    fn column_layout_single_column() {
        let elements = synth_body_pages(10, 612.0, 792.0, &[(100.0, 500.0)], 80.0, 720.0, 10.0);
        let s = build_stats(&elements);
        assert_eq!(s.column_layout.column_count, 1);
        assert!(s.column_layout.column_dividers.is_empty());
    }

    // --- 18. Two-column with clean gutter (gpt2-style) ------------------------
    #[test]
    fn column_layout_two_column_clean_gutter() {
        let elements = synth_body_pages(
            10,
            612.0,
            792.0,
            &[(100.0, 280.0), (310.0, 500.0)],
            80.0,
            720.0,
            10.0,
        );
        let s = build_stats(&elements);
        assert_eq!(s.column_layout.column_count, 2);
        assert_eq!(s.column_layout.column_dividers.len(), 1);
        let d = s.column_layout.column_dividers[0];
        assert!(
            d > 280.0 && d < 310.0,
            "divider {} not in inter-col range (280, 310)",
            d
        );
    }

    // --- 19. Two-column with full-width interruption (attention-style) --------
    #[test]
    fn column_layout_full_width_spanner_collapses_to_one() {
        let mut elements = synth_body_pages(
            10,
            612.0,
            792.0,
            &[(100.0, 280.0), (310.0, 500.0)],
            80.0,
            720.0,
            10.0,
        );
        // Pages 1-3 also have full-width abstract spanning [100, 500] across
        // the inter-col band. With heatmap counting page-presence, the gutter
        // accumulates ~3/10 of body density — well above drop_ratio=0.10.
        for p in 1..=3 {
            push_solid_body(&mut elements, p, 200.0, 600.0);
        }
        let s = build_stats(&elements);
        assert_eq!(
            s.column_layout.column_count, 1,
            "full-width spanner should defeat sharp-drop detection"
        );
    }

    // --- 20. Right-edge taper rejection (rfc-quic-style) ----------------------
    #[test]
    fn column_layout_right_edge_taper_rejected() {
        let mut elements = Vec::new();
        for p in 1..=10 {
            // body x=[100, 480], lighter density in [480, 510]
            let mut y = 80.0;
            while y < 720.0 {
                elements.push(make_element(p, 100.0, y, 380.0, 10.0, 10.0, 0));
                y += 14.0;
            }
            // sparse trailing density (every 3rd row)
            let mut y2 = 80.0;
            while y2 < 720.0 {
                if ((y2 as i32) / 14) % 3 == 0 {
                    elements.push(make_element(p, 480.0, y2, 30.0, 10.0, 10.0, 0));
                }
                y2 += 14.0;
            }
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        assert_eq!(
            s.column_layout.column_count, 1,
            "right-edge taper must not be detected as a column boundary"
        );
    }

    // --- 21. Indented-list rejection (Police-style) ---------------------------
    #[test]
    fn column_layout_indented_list_marker_rejected() {
        let mut elements = Vec::new();
        for p in 1..=10 {
            // narrow indented marker column (sparse) at x=[90, 115]
            let mut ym = 80.0;
            while ym < 720.0 {
                if ((ym as i32) / 14) % 3 == 0 {
                    elements.push(make_element(p, 90.0, ym, 25.0, 10.0, 10.0, 0));
                }
                ym += 14.0;
            }
            // full body at x=[120, 500]
            let mut y = 80.0;
            while y < 720.0 {
                elements.push(make_element(p, 120.0, y, 380.0, 10.0, 10.0, 0));
                y += 14.0;
            }
            elements.push(make_element(p, 599.9, 791.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        assert_eq!(
            s.column_layout.column_count, 1,
            "indented list marker must not be detected as a column boundary"
        );
    }

    // --- 22. DensityGrid populated and shape-correct --------------------------
    #[test]
    fn density_grid_shape_letter_page_at_default_cell_size() {
        let elements = synth_body_pages(10, 612.0, 792.0, &[(100.0, 500.0)], 80.0, 720.0, 10.0);
        let s = build_stats(&elements);
        assert_eq!(s.heatmap.cell_size, 8);
        assert_eq!(s.heatmap.cols, (612u32).div_ceil(8));
        assert_eq!(s.heatmap.rows, (792u32).div_ceil(8));
        assert_eq!(
            s.heatmap.cells.len(),
            (s.heatmap.rows * s.heatmap.cols) as usize
        );
        assert!(s.heatmap.cells.iter().any(|&v| v > 0));
    }

    // --- 23. DensityGrid sum semantics ----------------------------------------
    #[test]
    fn density_grid_uses_sum_not_mean_or_max() {
        // 10 pages, each with one bbox covering exactly x∈[0,8), y∈[0,8).
        let mut elements = Vec::new();
        for p in 1..=10 {
            elements.push(make_element(p, 0.0, 0.0, 8.0, 8.0, 10.0, 0));
            // anchor for page extent
            elements.push(make_element(p, 99.9, 99.9, 0.05, 0.05, 10.0, 0));
        }
        let s = build_stats(&elements);
        // Cell (0,0) should be sum of 8x8 = 64 input cells, each at value 10.
        // Total = 640 (matches the handoff's "8×8×10 = 640").
        let cell_00 = s.heatmap.cells[0];
        assert_eq!(cell_00, 640, "expected sum 640, got {}", cell_00);
        // All other cells in row 0 should be 0.
        for col in 1..s.heatmap.cols as usize {
            assert_eq!(
                s.heatmap.cells[col], 0,
                "cell (0,{col}) should be 0 (no bbox there)"
            );
        }
    }

    // --- 24. Block 01 smoke test still passes (analytics builder default) -----
    #[test]
    fn empty_input_returns_default_stats() {
        let s = build_stats(&[]);
        assert_eq!(s.source_pages, 0);
        assert_eq!(s.column_layout.column_count, 0);
        assert!(s.column_layout.column_dividers.is_empty());
        assert!(s.per_page_footer_y.is_empty());
    }
}
