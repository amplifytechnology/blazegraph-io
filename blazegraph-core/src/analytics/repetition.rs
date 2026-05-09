// Repetition primitives: for each normalized text token, the pages and Y-buckets
// where it appears. Used by `GeometryStats` to derive header/footer zones and
// exposed for diagnostic / data-science use.
//
// See `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`
// (Block 03). Block 03 fills in the observation and zone-derivation logic.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// Note: Block 03 will introduce `HashSet` when zone-derivation logic lands.
// Don't import it yet — keep the stub clean.

/// Y-bucket index for repetition detection. Granularity controlled by
/// downstream config (default plus-or-minus 3pt bucket width).
pub type YBucket = i32;

/// Document-level repetition primitive: for each normalized text token, the
/// pages and Y-buckets where it appears. Used by `GeometryStats` to derive
/// `header_zone`/`footer_zone` and exposed for diagnostic / data-science use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepetitionMap {
    pub by_text: HashMap<String, RepetitionRecord>,
}

/// Per-token repetition record. `occurrences` carries the raw page x
/// y-bucket pairs; the rest are derived rollups consumed by zone detection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepetitionRecord {
    /// The normalized text token this record describes.
    pub text: String,
    /// All observed occurrences in document order.
    pub occurrences: Vec<RepetitionOccurrence>,
    /// Number of distinct pages on which the token appears.
    pub distinct_pages: usize,
    /// `distinct_pages / total_pages`. A high ratio is a strong header/footer
    /// signal.
    pub page_ratio: f32,
    /// Herfindahl over the y-bucket histogram. Values near 1.0 mean the token
    /// always appears in the same y-bucket.
    pub y_concentration: f32,
    /// Most frequent y-bucket, if any occurrences were observed.
    pub dominant_y_bucket: Option<YBucket>,
}

/// One observation of a token at a particular page and y-bucket.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepetitionOccurrence {
    pub page: u32,
    pub y_bucket: YBucket,
}
