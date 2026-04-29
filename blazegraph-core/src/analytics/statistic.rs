// Core trait for the analytics pre-pass.
//
// See `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`.

use serde::{de::DeserializeOwned, Serialize};

use crate::analytics::geometry::GeometryStats;
use crate::types::PdfTextElement;

/// A single statistic kind that can observe text elements during the analytics
/// pre-pass and emit a finalized output. Implementors are stateful builders;
/// one instance per document analysis pass.
///
/// The trait shape mirrors Postgres `pg_statistic`: each kind is a
/// self-contained builder that observes elements one at a time and emits a
/// finalized output.
///
/// See `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`
/// for the full design.
pub trait Statistic {
    /// The serializable output type produced by `finalize()`.
    type Output: Serialize + DeserializeOwned + Clone + Default + std::fmt::Debug;

    /// Stable identifier for this statistic kind. Used for diagnostic output
    /// and future config-level enable/disable.
    const NAME: &'static str;

    /// Observe a single text element. Called once per element in the document,
    /// in reading order, during the single-pass walk. Implementations
    /// accumulate state.
    fn observe(&mut self, element: &PdfTextElement);

    /// Consume the builder and produce the finalized output. The context
    /// provides access to other already-finalized stat kinds when needed
    /// (e.g. PageStats reads finalized GeometryStats to know the band
    /// partitioning).
    fn finalize(self, ctx: &FinalizationContext<'_>) -> Self::Output;
}

/// Cross-stat dependencies available during finalization. Built incrementally
/// by `AnalysisBuilder::finalize()` so that stat kinds finalized later can
/// depend on stat kinds finalized earlier.
///
/// Stat kinds that have no dependencies receive an effectively-empty context.
#[derive(Debug, Default)]
pub struct FinalizationContext<'a> {
    /// Finalized geometry, available to stats finalized after geometry. `None`
    /// if finalization order has not yet reached geometry, or if geometry
    /// stats are disabled by config.
    pub geometry: Option<&'a GeometryStats>,
}
