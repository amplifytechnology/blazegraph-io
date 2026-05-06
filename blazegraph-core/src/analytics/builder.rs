// Composite builder that drives the single-pass walk and the dependency-ordered
// finalization. See
// `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`.

use serde::{Deserialize, Serialize};

use crate::analytics::font::{FontStats, FontStatsBuilder};
use crate::analytics::geometry::{GeometryStats, GeometryStatsBuilder};
use crate::analytics::page_stats::{PageStats, PageStatsBuilder};
use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::PdfTextElement;

/// Composite builder that drives a single-pass walk over text elements,
/// dispatching each element to all enabled stat kinds.
///
/// Construct with [`AnalysisBuilder::new`], call [`AnalysisBuilder::observe`]
/// once per element, then [`AnalysisBuilder::finalize`] to obtain a
/// [`DocumentAnalysis`].
#[derive(Debug, Default)]
pub struct AnalysisBuilder {
    pub font: FontStatsBuilder,
    pub geometry: GeometryStatsBuilder,
    pub page_stats: PageStatsBuilder,
}

impl AnalysisBuilder {
    /// Construct an empty builder ready to observe elements.
    pub fn new() -> Self {
        Self::default()
    }

    /// Dispatch an element to every enabled stat kind. Called once per element
    /// in document reading order.
    pub fn observe(&mut self, element: &PdfTextElement) {
        self.font.observe(element);
        self.geometry.observe(element);
        self.page_stats.observe(element);
    }

    /// Finalize all stat kinds in dependency order and produce a
    /// [`DocumentAnalysis`]. Font and geometry have no cross-stat
    /// dependencies; page_stats depends on the finalized geometry, so the
    /// geometry output is threaded into the context for the page_stats
    /// finalization.
    pub fn finalize(self) -> DocumentAnalysis {
        // Font has no dependencies.
        let empty_ctx = FinalizationContext::default();
        let font = self.font.finalize(&empty_ctx);

        // Geometry depends on the finalized FontStats — its per-page footer
        // walk reads the document-level body size instead of a fragile
        // per-page median. See `find_per_page_footer_line` in geometry.rs
        // for the rationale.
        let geometry_ctx = FinalizationContext {
            font: Some(&font),
            geometry: None,
        };
        let geometry = self.geometry.finalize(&geometry_ctx);

        // PageStats depends on the finalized GeometryStats and reads font
        // for the same reason geometry does.
        let page_ctx = FinalizationContext {
            font: Some(&font),
            geometry: Some(&geometry),
        };
        let page_stats = self.page_stats.finalize(&page_ctx);

        DocumentAnalysis {
            font,
            geometry,
            page_stats,
        }
    }
}

/// Composite output of the analytics pre-pass. Carries one finalized output
/// per stat kind. Lives in pipeline memory; not serialized into the public
/// graph output (a separate sidecar dump may serialize it for development
/// purposes).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentAnalysis {
    pub font: FontStats,
    pub geometry: GeometryStats,
    pub page_stats: PageStats,
}
