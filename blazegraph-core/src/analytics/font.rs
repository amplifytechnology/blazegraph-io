// Font-level descriptive statistics. Migrated from
// `DocumentAnalysis::analyze_text_elements()` in `types.rs` (Block 02 migration
// commit). Extended with cost-function primitives in the Block 02 extension commit.
//
// See `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`
// (Block 02) for the field semantics and the migration path.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::PdfTextElement;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tunable parameters for font-level statistics.
///
/// Defaults are baked in for now; YAML wiring lands in Block 05.
#[derive(Debug, Clone)]
pub struct FontStatsConfig {
    /// Number of top dominant font sizes (by element count) to retain in
    /// `top_k_dominant_sizes`. Default: 5.
    pub top_k: usize,
    /// Minimum fraction of elements (0.0–1.0) a font size must account for to
    /// appear in `distinct_size_frequency_profile`. Default: 0.05 (5%).
    pub distinct_size_min_fraction: f32,
}

impl Default for FontStatsConfig {
    fn default() -> Self {
        Self {
            top_k: 5,
            distinct_size_min_fraction: 0.05,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Convert a raw font size to the canonical string key used throughout
/// `FontStats`. The one-decimal-place format matches `types.rs` and must be
/// used by ALL callers (builder, consumers, tests) to ensure cache-compat.
fn size_key(size: f32) -> String {
    format!("{:.1}", size)
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for font-level descriptive statistics.
///
/// Constructed once per document. Call [`observe`] for every
/// [`PdfTextElement`] in reading order, then [`finalize`] to produce a
/// [`FontStats`].
#[derive(Debug, Default)]
pub struct FontStatsBuilder {
    // config is used in the extension commit (Block 02 extension); retained here
    // so the struct shape is stable across both commits.
    #[allow(dead_code)]
    config: FontStatsConfig,
    // Accumulators populated during observe().
    size_counts: BTreeMap<String, usize>,   // size_key → count
    family_counts: BTreeMap<String, usize>, // family → count
    bold_count: usize,
    non_bold_count: usize,
    italic_count: usize,
    non_italic_count: usize,
    bold_count_per_size: BTreeMap<String, usize>, // size_key → bold count at that size
    distinct_sizes: BTreeSet<u32>,                // f32 bit-pattern (f32::to_bits) for ordering
}

impl FontStatsBuilder {
    /// Construct a builder with explicit config. The `AnalysisBuilder` in
    /// `builder.rs` uses `FontStatsBuilder::default()` which picks up
    /// `FontStatsConfig::default()`.
    pub fn new(config: FontStatsConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }
}

impl Statistic for FontStatsBuilder {
    type Output = FontStats;
    const NAME: &'static str = "font";

    fn observe(&mut self, element: &PdfTextElement) {
        // Exclude rotated elements (rotation != 0) from font hierarchy
        // statistics. Rotated content (e.g. arxiv sidebar at rotation=90)
        // corrupts most_common_font_size with non-body-flow sizes. CR-10.
        if element.rotation() != 0 {
            return;
        }

        let style = &element.style_info;
        let key = size_key(style.font_size);

        // Size accumulation.
        *self.size_counts.entry(key.clone()).or_insert(0) += 1;
        self.distinct_sizes.insert(style.font_size.to_bits());

        // Family accumulation.
        *self
            .family_counts
            .entry(style.font_family.clone())
            .or_insert(0) += 1;

        // Bold / non-bold.
        let is_bold = style.font_weight.to_lowercase().contains("bold");
        if is_bold {
            self.bold_count += 1;
            *self.bold_count_per_size.entry(key).or_insert(0) += 1;
        } else {
            self.non_bold_count += 1;
        }

        // Italic / non-italic.
        let is_italic = style.font_style.to_lowercase().contains("italic");
        if is_italic {
            self.italic_count += 1;
        } else {
            self.non_italic_count += 1;
        }
    }

    fn finalize(self, _ctx: &FinalizationContext<'_>) -> Self::Output {
        // ----------------------------------------------------------------
        // Migrated outputs — field-by-field equivalence to the old
        // DocumentAnalysis::analyze_text_elements() on the overlapping shape.
        // The deterministic tiebreakers below are a FIX relative to the old
        // code, which used HashMap iteration order (undefined).
        // ----------------------------------------------------------------

        // most_common_font_size: size with highest count.
        // Deterministic tiebreaker: on equal count, LARGER size wins.
        let most_common_font_size = self
            .size_counts
            .iter()
            .max_by(|(key_a, &cnt_a), (key_b, &cnt_b)| {
                cnt_a.cmp(&cnt_b).then_with(|| {
                    let a: f32 = key_a.parse().unwrap_or(0.0);
                    let b: f32 = key_b.parse().unwrap_or(0.0);
                    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
                })
            })
            .and_then(|(key, _)| key.parse::<f32>().ok())
            .unwrap_or(12.0);

        // most_common_font_family: family with highest count.
        // Deterministic tiebreaker: on equal count, LEXICOGRAPHICALLY SMALLER
        // family name wins.
        let most_common_font_family = self
            .family_counts
            .iter()
            .max_by(|(name_a, &cnt_a), (name_b, &cnt_b)| {
                cnt_a.cmp(&cnt_b).then_with(|| name_b.cmp(name_a)) // reverse: smaller name wins
            })
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // all_font_sizes: sorted-ascending Vec<f32> of every distinct size seen.
        // BTreeSet<u32> over IEEE-754 bit patterns: for positive f32, integer
        // sort order equals numeric ascending order, so no further sort needed.
        let all_font_sizes: Vec<f32> = self
            .distinct_sizes
            .iter()
            .map(|&bits| f32::from_bits(bits))
            .collect();

        FontStats {
            font_size_counts: self.size_counts,
            font_family_counts: self.family_counts,
            bold_counts: BoldCounts {
                bold: self.bold_count,
                non_bold: self.non_bold_count,
            },
            italic_counts: ItalicCounts {
                italic: self.italic_count,
                non_italic: self.non_italic_count,
            },
            most_common_font_size,
            most_common_font_family,
            all_font_sizes,
            // Extension fields — populated in Block 02 extension commit.
            top_k_dominant_sizes: vec![],
            distinct_size_frequency_profile: vec![],
            size_gap_first_to_second: 0.0,
            bold_density_per_size: BTreeMap::new(),
            mean_font_size: 0.0,
            median_font_size: 0.0,
            variance_font_size: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// Document-level font statistics produced by the analytics pre-pass. All
/// fields are descriptive primitives — they expose font-distribution shape for
/// the corpus-analysis lab and for downstream rules. Classification decisions
/// (heading vs body, structural significance) belong to consumer rules, not to
/// this type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FontStats {
    /// Element-count histogram keyed by string-formatted font size (see
    /// `size_key()` helper). Deterministic key ordering via `BTreeMap`.
    pub font_size_counts: BTreeMap<String, usize>,
    /// Element-count histogram keyed by font family name. Deterministic key
    /// ordering via `BTreeMap`.
    pub font_family_counts: BTreeMap<String, usize>,
    /// Element counts split by bold/non-bold.
    pub bold_counts: BoldCounts,
    /// Element counts split by italic/non-italic.
    pub italic_counts: ItalicCounts,
    /// Single canonical body-size signal: the font size with the largest
    /// element count. Deterministic tiebreaker: larger size wins on equal
    /// count. Defaults to 12.0 if no elements observed.
    pub most_common_font_size: f32,
    /// Single canonical body-family signal: the font family with the largest
    /// element count. Deterministic tiebreaker: lexicographically smaller
    /// family name wins on equal count. Defaults to `"unknown"` if no
    /// elements observed.
    pub most_common_font_family: String,
    /// Sorted, deduplicated list of every distinct font size seen (ascending).
    pub all_font_sizes: Vec<f32>,
    /// Top K font sizes by element count. Descriptive primitive only. Dominant
    /// sizes are typically body text; rare/non-dominant sizes are NOT
    /// necessarily structurally meaningful (e.g., a rare large red callout is
    /// not a heading). Consumer rules combine this with isolation, position,
    /// and other signals before drawing structural conclusions.
    pub top_k_dominant_sizes: Vec<f32>,
    /// Font sizes that account for at least `config.distinct_size_min_fraction`
    /// of elements. Descriptive primitive — populates corpus analysis and
    /// downstream rules. The number of qualifying sizes is suggestive of
    /// effective font diversity but is NOT a heading-depth classifier (rare
    /// sizes are often inline emphasis, formula symbols, or non-structural
    /// callouts).
    pub distinct_size_frequency_profile: Vec<DistinctSizeFrequency>,
    /// Absolute size difference between the two most common sizes (by element
    /// count). A small gap suggests low size-based separability between dominant
    /// content streams; a large gap suggests one stream is markedly larger than
    /// the other. Not a structural classifier on its own.
    pub size_gap_first_to_second: f32,
    /// Per-size fraction of elements that are bold. Identifies sizes where bold
    /// occurs disproportionately, candidate for the "bold heading size" signal.
    /// Combined with isolation and position by consumer rules.
    pub bold_density_per_size: BTreeMap<String, f32>,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BoundingBox, FontClass, Placement};

    /// Build a minimal `PdfTextElement` for unit tests.
    /// Bbox/page/band/column can be zeros — FontStats does not read them.
    fn make_element(
        text: &str,
        size: f32,
        family: &str,
        weight: &str,
        style: &str,
        rotation: i32,
    ) -> PdfTextElement {
        PdfTextElement {
            text: text.to_string(),
            style_info: FontClass {
                class_name: "test".to_string(),
                font_family: family.to_string(),
                font_size: size,
                font_style: style.to_string(),
                font_weight: weight.to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: 0,
                bounding_box: BoundingBox {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: size,
                },
                band: 0,
                column: 0,
                nr_band_columns: 1,
                line_number: 0,
                segment_number: 0,
                rotation,
                paragraph_number: 0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
        }
    }

    fn build_stats(elements: &[PdfTextElement]) -> FontStats {
        let mut b = FontStatsBuilder::default();
        for e in elements {
            b.observe(e);
        }
        b.finalize(&FinalizationContext::default())
    }

    // -----------------------------------------------------------------------
    // MIGRATION COMMIT TESTS
    // -----------------------------------------------------------------------

    /// Equivalence test for migration. Delete when the old
    /// DocumentAnalysis::analyze_text_elements() is removed in Block 05.
    ///
    /// Covers: 2 sizes × 2 families × bold/non-bold × italic/non-italic ×
    /// rotation 0/90 mix, plus an empty-elements case.
    ///
    /// NOTE: Inputs are constructed to avoid ties on most_common_font_size
    /// and most_common_font_family so the equivalence assertion is stable.
    /// Tied inputs are covered separately in `test_deterministic_tiebreaking`.
    #[test]
    fn test_migration_equivalence() {
        use crate::types::DocumentAnalysis as LegacyDocumentAnalysis;

        // -- Empty input --
        {
            let elements: Vec<PdfTextElement> = vec![];
            let old = LegacyDocumentAnalysis::analyze_text_elements(&elements);
            let new = build_stats(&elements);

            assert_eq!(
                old.font_size_counts,
                new.font_size_counts
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect::<std::collections::HashMap<_, _>>()
            );
            assert_eq!(
                old.font_family_counts,
                new.font_family_counts
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect::<std::collections::HashMap<_, _>>()
            );
            assert_eq!(old.bold_counts.0, new.bold_counts.bold);
            assert_eq!(old.bold_counts.1, new.bold_counts.non_bold);
            assert_eq!(old.italic_counts.0, new.italic_counts.italic);
            assert_eq!(old.italic_counts.1, new.italic_counts.non_italic);
            assert_eq!(old.most_common_font_size, new.most_common_font_size);
            assert_eq!(old.most_common_font_family, new.most_common_font_family);
            assert_eq!(old.all_font_sizes, new.all_font_sizes);
        }

        // -- Mixed input (2 sizes, 2 families, bold/non-bold, italic/non-italic,
        //    rotation 0/90 mix) --
        // Sizes: 12pt × 9 elements, 14pt × 3 elements → 12pt wins most_common.
        // Families: "Arial" × 9, "Times" × 3 → Arial wins most_common.
        // Rotated: 2 elements at rotation=90 (must be excluded).
        {
            let mut elements: Vec<PdfTextElement> = vec![];
            // 8 × 12pt, Arial, normal weight, normal style, rotation=0
            for _ in 0..8 {
                elements.push(make_element("body", 12.0, "Arial", "normal", "normal", 0));
            }
            // 3 × 14pt, Times, bold, normal style, rotation=0
            for _ in 0..3 {
                elements.push(make_element("header", 14.0, "Times", "bold", "normal", 0));
            }
            // 1 × 12pt, Arial, bold, italic, rotation=0 → counts in bold+italic
            elements.push(make_element(
                "bolditalic",
                12.0,
                "Arial",
                "bold",
                "italic",
                0,
            ));
            // 2 × 20pt, OtherFamily, normal, normal, rotation=90 → excluded
            for _ in 0..2 {
                elements.push(make_element(
                    "rotated",
                    20.0,
                    "OtherFamily",
                    "normal",
                    "normal",
                    90,
                ));
            }

            let old = LegacyDocumentAnalysis::analyze_text_elements(&elements);
            let new = build_stats(&elements);

            let old_size_map: std::collections::HashMap<String, usize> =
                old.font_size_counts.clone();
            let new_size_map: std::collections::HashMap<String, usize> = new
                .font_size_counts
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            assert_eq!(old_size_map, new_size_map, "font_size_counts mismatch");

            let old_fam_map: std::collections::HashMap<String, usize> =
                old.font_family_counts.clone();
            let new_fam_map: std::collections::HashMap<String, usize> = new
                .font_family_counts
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            assert_eq!(old_fam_map, new_fam_map, "font_family_counts mismatch");

            assert_eq!(old.bold_counts.0, new.bold_counts.bold, "bold count");
            assert_eq!(
                old.bold_counts.1, new.bold_counts.non_bold,
                "non_bold count"
            );
            assert_eq!(
                old.italic_counts.0, new.italic_counts.italic,
                "italic count"
            );
            assert_eq!(
                old.italic_counts.1, new.italic_counts.non_italic,
                "non_italic count"
            );
            assert_eq!(
                old.most_common_font_size, new.most_common_font_size,
                "most_common_font_size"
            );
            assert_eq!(
                old.most_common_font_family, new.most_common_font_family,
                "most_common_font_family"
            );
            assert_eq!(old.all_font_sizes, new.all_font_sizes, "all_font_sizes");
        }
    }

    /// Test 2 — Deterministic tiebreaking.
    ///
    /// Input: two sizes at equal count (12pt × 5, 14pt × 5).
    /// New builder must always resolve to 14.0 (larger wins on tie).
    /// Two runs on identical input must produce byte-identical JSON.
    #[test]
    fn test_deterministic_tiebreaking() {
        let mut elements: Vec<PdfTextElement> = vec![];
        for _ in 0..5 {
            elements.push(make_element("a", 12.0, "Arial", "normal", "normal", 0));
        }
        for _ in 0..5 {
            elements.push(make_element("b", 14.0, "Arial", "normal", "normal", 0));
        }

        let run1 = build_stats(&elements);
        let run2 = build_stats(&elements);

        assert_eq!(
            run1.most_common_font_size, 14.0,
            "larger size wins on tie: expected 14.0"
        );
        assert_eq!(
            run2.most_common_font_size, 14.0,
            "second run must also resolve to 14.0"
        );

        let json1 = serde_json::to_string(&run1).expect("serialize run1");
        let json2 = serde_json::to_string(&run2).expect("serialize run2");
        assert_eq!(json1, json2, "two runs must produce byte-identical JSON");
    }

    /// Test 9 — Rotation filter preserved.
    ///
    /// 5 elements at rotation=0, 3 at rotation=90. FontStats must count only
    /// the 5 non-rotated elements.
    #[test]
    fn test_rotation_filter_preserved() {
        let mut elements: Vec<PdfTextElement> = vec![];
        for _ in 0..5 {
            elements.push(make_element("body", 12.0, "Arial", "normal", "normal", 0));
        }
        for _ in 0..3 {
            elements.push(make_element(
                "rotated",
                20.0,
                "OtherFamily",
                "normal",
                "normal",
                90,
            ));
        }

        let stats = build_stats(&elements);

        assert_eq!(
            stats.font_size_counts.get("12.0"),
            Some(&5),
            "12pt should have count 5"
        );
        assert_eq!(
            stats.font_size_counts.get("20.0"),
            None,
            "rotated 20pt should not appear"
        );
        assert_eq!(
            stats.bold_counts.bold + stats.bold_counts.non_bold,
            5,
            "total element count should be 5"
        );
    }
}
