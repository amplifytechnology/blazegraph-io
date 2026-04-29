// Font-level descriptive statistics. Block 02 fills in the observation and
// finalization logic; this block defines the type shape only.
//
// See `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`
// (Block 02) for the field semantics and the migration path from the existing
// `DocumentAnalysis::analyze_text_elements()` impl in `types.rs`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::PdfTextElement;

/// Builder for font-level descriptive statistics. Block 02 populates the
/// observation logic. This block defines the type shape only.
#[derive(Debug, Default)]
pub struct FontStatsBuilder {
    // Block 02 will add accumulator fields here.
}

impl Statistic for FontStatsBuilder {
    type Output = FontStats;
    const NAME: &'static str = "font";

    fn observe(&mut self, _element: &PdfTextElement) {
        // Stub — Block 02 implements.
    }

    fn finalize(self, _ctx: &FinalizationContext<'_>) -> Self::Output {
        FontStats::default()
    }
}

/// Finalized font statistics. Field set is the shape Block 02 will populate.
/// All fields default to empty/zero in this block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FontStats {
    /// Element-count histogram keyed by string-formatted font size.
    pub font_size_counts: HashMap<String, usize>,
    /// Element-count histogram keyed by font family name.
    pub font_family_counts: HashMap<String, usize>,
    /// Element counts split by bold/non-bold.
    pub bold_counts: BoldCounts,
    /// Element counts split by italic/non-italic.
    pub italic_counts: ItalicCounts,
    /// Single canonical body-size signal: the font size with the largest
    /// element count.
    pub most_common_font_size: f32,
    /// Single canonical body-family signal: the font family with the largest
    /// element count.
    pub most_common_font_family: String,
    /// Sorted, deduplicated list of every font size seen.
    pub all_font_sizes: Vec<f32>,
    /// First N sizes by element count (default N = 5). Useful as a prior; not
    /// a direct mapping to heading levels.
    pub top_k_dominant_sizes: Vec<f32>,
    /// Distinct font sizes that account for at least a configured fraction of
    /// the document's elements.
    pub distinct_size_frequency_profile: Vec<DistinctSizeFrequency>,
    /// Separability score between the dominant size and the next most
    /// frequent. Useful prior; not a classification by itself.
    pub size_gap_first_to_second: f32,
    /// Per-size bold ratio: how strongly bold concentrates at each font size.
    pub bold_density_per_size: HashMap<String, f32>,
    /// Mean of font sizes weighted by element count.
    pub mean_font_size: f32,
    /// Median of font sizes weighted by element count.
    pub median_font_size: f32,
    /// Variance of font sizes weighted by element count.
    pub variance_font_size: f32,
}

/// Element counts split by bold/non-bold.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoldCounts {
    pub bold: usize,
    pub non_bold: usize,
}

/// Element counts split by italic/non-italic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItalicCounts {
    pub italic: usize,
    pub non_italic: usize,
}

/// One row of `FontStats::distinct_size_frequency_profile`: a font size that
/// accounts for `fraction_of_elements` of the document's elements.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DistinctSizeFrequency {
    pub size_key: String,
    pub fraction_of_elements: f32,
}
