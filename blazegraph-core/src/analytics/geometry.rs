// Per-page and per-document geometric structure: bands, columns, dominant
// column count, header/footer Y-zones. Block 03 fills in the observation and
// finalization logic; this block defines the type shape only.
//
// See `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`
// (Block 03) for the inference algorithms and the highest-residual-ambiguity
// notes.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::analytics::repetition::RepetitionMap;
use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::PdfTextElement;

/// Builder for geometry-level descriptive statistics. Block 03 populates the
/// observation logic. This block defines the type shape only.
#[derive(Debug, Default)]
pub struct GeometryStatsBuilder {
    // Block 03 will add accumulator fields here.
}

impl Statistic for GeometryStatsBuilder {
    type Output = GeometryStats;
    const NAME: &'static str = "geometry";

    fn observe(&mut self, _element: &PdfTextElement) {
        // Stub — Block 03 implements.
    }

    fn finalize(self, _ctx: &FinalizationContext<'_>) -> Self::Output {
        GeometryStats::default()
    }
}

/// Document-level geometric statistics. Carries per-page geometry, dominant
/// column count, the band-column histogram, and the document-level
/// repetition primitives used to derive header/footer zones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeometryStats {
    /// One entry per page in document order.
    pub pages: Vec<PageGeometry>,
    /// Mode of band-column counts weighted by element count.
    pub dominant_column_count: u32,
    /// Histogram of band-column counts across the document.
    pub band_column_histogram: HashMap<u32, usize>,
    /// Document-level repetition primitive: text x y-bucket x pages. Used to
    /// derive header/footer zones; exposed for diagnostic use.
    pub repetition: RepetitionMap,
}

/// Geometry for a single page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageGeometry {
    /// 1-indexed page number on the source PDF.
    pub page_number: u32,
    /// Page width in points.
    pub width: f32,
    /// Page height in points.
    pub height: f32,
    /// Bands on this page, in vertical order.
    pub bands: Vec<BandGeometry>,
    /// Y-range covered by the running header on this page, if detected.
    pub header_zone: Option<YRange>,
    /// Y-range covered by the running footer on this page, if detected.
    pub footer_zone: Option<YRange>,
}

/// Geometry for a single band on a page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BandGeometry {
    /// 0-indexed band position on the page.
    pub band_index: u32,
    /// Column count Tika reported for this band.
    pub tika_nr_columns: u32,
    /// Column count the pre-pass concludes after cross-checking against the
    /// document-level dominant pattern.
    pub inferred_nr_columns: u32,
    /// Vertical extent of the band.
    pub y_range: YRange,
    /// Horizontal extent of the band.
    pub x_extent: XExtent,
    /// Number of elements in this band.
    pub element_count: usize,
}

/// Inclusive vertical range, in points.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct YRange {
    pub min: f32,
    pub max: f32,
}

/// Inclusive horizontal range, in points.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct XExtent {
    pub min: f32,
    pub max: f32,
}
