// Token, element, and density distributions partitioned by the geometry from
// Block 03, plus document-level rollups. Block 04 fills in the observation and
// finalization logic; this block defines the type shape only.
//
// See `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`
// (Block 04) for the rationale and the "token counting composes with
// geometry" insight.

use serde::{Deserialize, Serialize};

use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::PdfTextElement;

/// Builder for page-level descriptive statistics. Block 04 populates the
/// observation logic. This block defines the type shape only.
#[derive(Debug, Default)]
pub struct PageStatsBuilder {
    // Block 04 will add accumulator fields here.
}

impl Statistic for PageStatsBuilder {
    type Output = PageStats;
    const NAME: &'static str = "page_stats";

    fn observe(&mut self, _element: &PdfTextElement) {
        // Stub — Block 04 implements.
    }

    fn finalize(self, _ctx: &FinalizationContext<'_>) -> Self::Output {
        PageStats::default()
    }
}

/// Document-level page statistics: per-page partitioned distributions plus a
/// document-wide rollup of cross-page metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageStats {
    /// One entry per page in document order.
    pub per_page: Vec<PageStatsPerPage>,
    /// Mean / median / variance / quantiles of cross-page metrics.
    pub document_rollup: DocumentStatsRollup,
}

/// Per-page partitioned distributions: tokens and elements split by band,
/// column, and Y-zone (header / body / footer).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageStatsPerPage {
    /// 1-indexed page number on the source PDF.
    pub page_number: u32,
    /// Total tokens on the page across all bands.
    pub total_tokens: usize,
    /// Total elements on the page across all bands.
    pub total_elements: usize,
    /// Per-band token / element / density distributions.
    pub by_band: Vec<BandStats>,
    /// Per-column token / element distributions.
    pub by_column: Vec<ColumnStats>,
    /// Header / body / footer split.
    pub by_y_zone: ZoneStats,
}

/// Token, element, and density signals for a single band on a page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BandStats {
    /// 0-indexed band position on the page.
    pub band_index: u32,
    /// Total tokens in the band.
    pub token_count: usize,
    /// Total elements in the band.
    pub element_count: usize,
    /// Tokens divided by band area in points squared.
    pub density: f32,
    /// Fraction of elements in the band that are bold.
    pub bold_ratio: f32,
    /// Fraction of characters in the band that are alphabetic.
    pub alpha_ratio: f32,
}

/// Token / element distribution for a single column on a page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColumnStats {
    /// 0-indexed column position within the band.
    pub column_index: u32,
    pub token_count: usize,
    pub element_count: usize,
}

/// Header / body / footer split for a page, using Y-zones from
/// `PageGeometry`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneStats {
    pub header: ZoneTokens,
    pub body: ZoneTokens,
    pub footer: ZoneTokens,
}

/// Token / element tally for a single Y-zone (header / body / footer).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneTokens {
    pub token_count: usize,
    pub element_count: usize,
}

/// Document-level rollups across pages: mean / median / variance / quantiles
/// of per-page metrics. Useful for spotting cover/title/appendix pages by
/// their distinct profiles.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentStatsRollup {
    pub mean_tokens_per_page: f32,
    pub median_tokens_per_page: f32,
    pub variance_tokens_per_page: f32,
    pub p25_tokens_per_page: f32,
    pub p75_tokens_per_page: f32,
    pub p95_tokens_per_page: f32,
    pub mean_elements_per_page: f32,
    pub mean_font_diversity_per_page: f32,
    pub mean_bold_ratio_per_page: f32,
    pub mean_band_fill_ratio_per_page: f32,
}
